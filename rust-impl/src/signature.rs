/*!
BLS signatures
*/

use ff::Field;
use hkdf::Hkdf;
use pairing::bls12_381::{Bls12, Fq12, Fr, G1, G2};
use pairing::hash_to_curve::HashToCurve;
use pairing::hash_to_field::BaseFromRO;
use pairing::serdes::SerDes;
use pairing::{CurveAffine, CurveProjective, Engine};
use sha2::digest::generic_array::typenum::U48;
use sha2::digest::generic_array::GenericArray;
use sha2::Sha256;
use std::collections::HashSet;
use std::io::Cursor;
use std::vec::Vec;

/// Hash a secret key sk to the secret exponent x'; then (PK, SK) = (g^{x'}, x').
pub fn xprime_from_sk<B: AsRef<[u8]>>(msg: B) -> Fr {
    let mut result = GenericArray::<u8, U48>::default();
    // `result` has enough length to hold the output from HKDF expansion
    assert!(Hkdf::<Sha256>::new(None, msg.as_ref())
        .expand(&[], &mut result)
        .is_ok());
    Fr::from_okm(&result)
}

// multi-point-addition helper: used in aggregate and in PoP verify
fn _agg_help<T: CurveProjective>(ins: &[T]) -> T {
    let mut ret = T::zero();
    for inv in ins {
        ret.add_assign(inv);
    }
    ret
}

/// Alias for the scalar type corresponding to a CurveProjective type
type ScalarT<PtT> = <PtT as CurveProjective>::Scalar;

/// BLS signature implementation
pub trait BLSSigCore: CurveProjective {
    /// The type of the public key
    type PKType: CurveProjective<Engine = <Self as CurveProjective>::Engine, Scalar = ScalarT<Self>>;

    /// Generate secret exponent and public key
    /// * input: the secret key as bytes
    /// * output: the actual secret key x_prime, a.k.a, the secret scala
    /// * output: the public key g^x_prime
    fn keygen<B: AsRef<[u8]>>(sk: B) -> (ScalarT<Self>, Self::PKType);

    /// Sign a message
    /// * input: the actual secret key x_prime
    /// * input: the message as bytes
    /// * input: the ciphersuite ID
    /// * output: a signature
    fn core_sign<B: AsRef<[u8]>>(x_prime: ScalarT<Self>, msg: B, ciphersuite: u8) -> Self;

    /// Verify a signature
    /// * input: public key, a group element
    /// * input: signature, a group element
    /// * input: the message as bytes
    /// * input: ciphersuite ID
    /// * output: if the signature is valid or not
    fn core_verify<B: AsRef<[u8]>>(pk: Self::PKType, sig: Self, msg: B, ciphersuite: u8) -> bool;

    /// Aggregate signatures
    fn aggregate(sigs: &[Self]) -> Self {
        _agg_help(sigs)
    }

    /// Verify an aggregated signature
    fn core_aggregate_verify<B: AsRef<[u8]>>(
        pks: &[Self::PKType],
        msgs: &[B],
        sig: Self,
        ciphersuite: u8,
    ) -> bool;
}

/// 'Basic' BLS signature
pub trait BLSSignatureBasic: BLSSigCore {
    /// re-export from BLSSigCore
    fn sign<B: AsRef<[u8]>>(x_prime: ScalarT<Self>, msg: B, ciphersuite: u8) -> Self {
        <Self as BLSSigCore>::core_sign(x_prime, msg, ciphersuite)
    }

    /// re-export from BLSSigCore
    fn verify<B: AsRef<[u8]>>(pk: Self::PKType, sig: Self, msg: B, ciphersuite: u8) -> bool {
        <Self as BLSSigCore>::core_verify(pk, sig, msg, ciphersuite)
    }

    /// check for uniqueness of msgs, then invoke verify from BLSSigCore
    fn aggregate_verify<B: AsRef<[u8]>>(
        pks: &[Self::PKType],
        msgs: &[B],
        sig: Self,
        ciphersuite: u8,
    ) -> bool {
        // enforce uniqueness of messages
        let mut msg_set = HashSet::<&[u8]>::with_capacity(msgs.len());
        for msg in msgs {
            msg_set.insert(msg.as_ref());
        }
        if msg_set.len() != msgs.len() {
            return false;
        }

        <Self as BLSSigCore>::core_aggregate_verify(pks, msgs, sig, ciphersuite)
    }
}

/// BLS signature with message augmentation
pub trait BLSSignatureAug: BLSSigCore {
    /// turn a public key into a vector
    fn pk_bytes(pk: &Self::PKType, size_hint: usize) -> Vec<u8>;

    /// augment message and then invoke coresign
    fn sign<B: AsRef<[u8]>>(x_prime: ScalarT<Self>, msg: B, ciphersuite: u8) -> Self {
        let pk = {
            let mut tmp = <Self::PKType as CurveProjective>::one();
            tmp.mul_assign(x_prime);
            tmp
        };
        let mut pk_msg_vec = Self::pk_bytes(&pk, msg.as_ref().len());
        pk_msg_vec.extend_from_slice(msg.as_ref());
        <Self as BLSSigCore>::core_sign(x_prime, &pk_msg_vec, ciphersuite)
    }

    /// augment message and then invoke coreverify
    fn verify<B: AsRef<[u8]>>(pk: Self::PKType, sig: Self, msg: B, ciphersuite: u8) -> bool {
        let mut pk_msg_vec = Self::pk_bytes(&pk, msg.as_ref().len());
        pk_msg_vec.extend_from_slice(msg.as_ref());
        <Self as BLSSigCore>::core_verify(pk, sig, &pk_msg_vec, ciphersuite)
    }

    /// augment all messages and then invoke coreverify
    fn aggregate_verify<B: AsRef<[u8]>>(
        pks: &[Self::PKType],
        msgs: &[B],
        sig: Self,
        ciphersuite: u8,
    ) -> bool {
        let mut pks_msgs_vec = Vec::<Vec<u8>>::with_capacity(msgs.len());
        for (msg, pk) in msgs.as_ref().iter().zip(pks) {
            let mut pk_msg_vec = Self::pk_bytes(&pk, msg.as_ref().len());
            pk_msg_vec.extend_from_slice(msg.as_ref());
            pks_msgs_vec.push(pk_msg_vec);
        }
        <Self as BLSSigCore>::core_aggregate_verify(pks, &pks_msgs_vec[..], sig, ciphersuite)
    }
}

impl<T> BLSSignatureAug for T
where
    T: BLSSigCore,
    <T as BLSSigCore>::PKType: SerDes,
{
    fn pk_bytes(pk: &Self::PKType, size_hint: usize) -> Vec<u8> {
        // 96 bytes of overhead for the PK, plus the size hint
        let mut cur = Cursor::new(Vec::<u8>::with_capacity(size_hint + 96));
        assert!(pk.serialize(&mut cur, true).is_ok());
        cur.into_inner()
    }
}

/// BLS signature with proof of possession
pub trait BLSSignaturePop: BLSSigCore {
    /// re-export from BLSSigCore
    fn sign<B: AsRef<[u8]>>(x_prime: ScalarT<Self>, msg: B, ciphersuite: u8) -> Self {
        <Self as BLSSigCore>::core_sign(x_prime, msg, ciphersuite)
    }

    /// re-export from BLSSigCore
    fn verify<B: AsRef<[u8]>>(pk: Self::PKType, sig: Self, msg: B, ciphersuite: u8) -> bool {
        <Self as BLSSigCore>::core_verify(pk, sig, msg, ciphersuite)
    }

    /// just invoke verify from BLSSigCore
    fn aggregate_verify<B: AsRef<[u8]>>(
        pks: &[Self::PKType],
        msgs: &[B],
        sig: Self,
        ciphersuite: u8,
    ) -> bool {
        <Self as BLSSigCore>::core_aggregate_verify(pks, msgs, sig, ciphersuite)
    }

    /// verify a multisig
    fn multisig_verify<B: AsRef<[u8]>>(
        pks: &[Self::PKType],
        sig: Self,
        msg: B,
        ciphersuite: u8,
    ) -> bool {
        let apk = _agg_help(pks);
        <Self as BLSSigCore>::core_verify(apk, sig, msg, ciphersuite)
    }

    /// prove possession
    fn pop_prove<B: AsRef<[u8]>>(sk: B, ciphersuite: u8) -> Self;

    /// prove possession where both keys are present
    fn pop_prove_with_both_keys(
        sk: &Fr,
        pk: &<Self as BLSSigCore>::PKType,
        ciphersuite: u8,
    ) -> Self;

    /// check proof of possession
    fn pop_verify(pk: <Self as BLSSigCore>::PKType, sig: Self, ciphersuite: u8) -> bool;
}

impl BLSSigCore for G1 {
    type PKType = G2;

    fn keygen<B: AsRef<[u8]>>(sk: B) -> (Fr, G2) {
        let x_prime = xprime_from_sk(sk);
        let mut pk = G2::one();
        pk.mul_assign(x_prime);
        (x_prime, pk)
    }

    fn core_sign<B: AsRef<[u8]>>(x_prime: Fr, msg: B, ciphersuite: u8) -> G1 {
        let mut p = G1::hash_to_curve(msg, ciphersuite);
        p.mul_assign(x_prime);
        p
    }

    fn core_verify<B: AsRef<[u8]>>(pk: G2, sig: G1, msg: B, ciphersuite: u8) -> bool {
        let p = G1::hash_to_curve(msg, ciphersuite).into_affine().prepare();
        let g2gen = {
            let mut tmp = G2::one();
            tmp.negate();
            tmp.into_affine().prepare()
        };

        match Bls12::final_exponentiation(&Bls12::miller_loop(&[
            (&p, &pk.into_affine().prepare()),
            (&sig.into_affine().prepare(), &g2gen),
        ])) {
            None => false,
            Some(pairingproduct) => pairingproduct == Fq12::one(),
        }
    }

    fn core_aggregate_verify<B: AsRef<[u8]>>(
        pks: &[G2],
        msgs: &[B],
        sig: G1,
        ciphersuite: u8,
    ) -> bool {
        let pvec = {
            let mut ret =
                Vec::<<<G1 as CurveProjective>::Affine as CurveAffine>::Prepared>::with_capacity(
                    msgs.len() + 1,
                );
            for msg in msgs {
                ret.push(G1::hash_to_curve(msg, ciphersuite).into_affine().prepare());
            }
            ret.push(sig.into_affine().prepare());
            ret
        };
        let qvec = {
            let mut ret =
                Vec::<<<G2 as CurveProjective>::Affine as CurveAffine>::Prepared>::with_capacity(
                    pks.len() + 1,
                );
            for pk in pks {
                ret.push(pk.into_affine().prepare());
            }
            let mut tmp = G2::one();
            tmp.negate();
            ret.push(tmp.into_affine().prepare());
            ret
        };

        // XXX: this is annoying: miller_loop requires an iter to tuple refs, not tuples
        let pqz: Vec<_> = pvec.as_slice().iter().zip(qvec.as_slice()).collect();
        match Bls12::final_exponentiation(&Bls12::miller_loop(&pqz[..])) {
            None => false,
            Some(pairingproduct) => pairingproduct == Fq12::one(),
        }
    }
}

impl BLSSignaturePop for G1 {
    fn pop_prove<B: AsRef<[u8]>>(sk: B, ciphersuite: u8) -> G1 {
        let (x_prime, pk) = G1::keygen(sk);
        let pk_bytes = {
            let mut buf = [0u8; 96];
            let mut cur = Cursor::new(&mut buf[..]);
            assert!(pk.serialize(&mut cur, true).is_ok());
            buf
        };
        G1::core_sign(x_prime, &pk_bytes[..], ciphersuite)
    }

    fn pop_prove_with_both_keys(sk: &Fr, pk: &G2, ciphersuite: u8) -> G1 {
        let pk_bytes = {
            let mut buf = [0u8; 96];
            let mut cur = Cursor::new(&mut buf[..]);
            assert!(pk.serialize(&mut cur, true).is_ok());
            buf
        };
        G1::core_sign(*sk, &pk_bytes[..], ciphersuite)
    }

    fn pop_verify(pk: G2, sig: G1, ciphersuite: u8) -> bool {
        let pk_bytes = {
            let mut buf = [0u8; 96];
            let mut cur = Cursor::new(&mut buf[..]);
            assert!(pk.serialize(&mut cur, true).is_ok());
            buf
        };
        G1::core_verify(pk, sig, &pk_bytes[..], ciphersuite)
    }
}

impl BLSSigCore for G2 {
    type PKType = G1;

    fn keygen<B: AsRef<[u8]>>(sk: B) -> (Fr, G1) {
        let x_prime = xprime_from_sk(sk);
        let mut pk = G1::one();
        pk.mul_assign(x_prime);
        (x_prime, pk)
    }

    fn core_sign<B: AsRef<[u8]>>(x_prime: Fr, msg: B, ciphersuite: u8) -> G2 {
        let mut p = G2::hash_to_curve(msg, ciphersuite);
        p.mul_assign(x_prime);
        p
    }

    fn core_verify<B: AsRef<[u8]>>(pk: G1, sig: G2, msg: B, ciphersuite: u8) -> bool {
        let p = G2::hash_to_curve(msg, ciphersuite).into_affine().prepare();
        let g1gen = {
            let mut tmp = G1::one();
            tmp.negate();
            tmp.into_affine().prepare()
        };

        match Bls12::final_exponentiation(&Bls12::miller_loop(&[
            (&pk.into_affine().prepare(), &p),
            (&g1gen, &sig.into_affine().prepare()),
        ])) {
            None => false,
            Some(pairingproduct) => pairingproduct == Fq12::one(),
        }
    }

    fn core_aggregate_verify<B: AsRef<[u8]>>(
        pks: &[G1],
        msgs: &[B],
        sig: G2,
        ciphersuite: u8,
    ) -> bool {
        let pvec = {
            let mut ret =
                Vec::<<<G1 as CurveProjective>::Affine as CurveAffine>::Prepared>::with_capacity(
                    pks.len() + 1,
                );
            for pk in pks {
                ret.push(pk.into_affine().prepare());
            }
            let mut tmp = G1::one();
            tmp.negate();
            ret.push(tmp.into_affine().prepare());
            ret
        };
        let qvec = {
            let mut ret =
                Vec::<<<G2 as CurveProjective>::Affine as CurveAffine>::Prepared>::with_capacity(
                    msgs.len() + 1,
                );
            for msg in msgs {
                ret.push(G2::hash_to_curve(msg, ciphersuite).into_affine().prepare());
            }
            ret.push(sig.into_affine().prepare());
            ret
        };

        // XXX: this is annoying: miller_loop requires an iter to tuple refs, not tuples
        let pqz: Vec<_> = pvec.as_slice().iter().zip(qvec.as_slice()).collect();
        match Bls12::final_exponentiation(&Bls12::miller_loop(&pqz[..])) {
            None => false,
            Some(pairingproduct) => pairingproduct == Fq12::one(),
        }
    }
}

impl BLSSignaturePop for G2 {
    fn pop_prove<B: AsRef<[u8]>>(sk: B, ciphersuite: u8) -> G2 {
        let (x_prime, pk) = G2::keygen(sk);
        let pk_bytes = {
            let mut buf = [0u8; 48];
            let mut cur = Cursor::new(&mut buf[..]);
            assert!(pk.serialize(&mut cur, true).is_ok());
            buf
        };
        G2::core_sign(x_prime, &pk_bytes[..], ciphersuite)
    }

    fn pop_prove_with_both_keys(sk: &Fr, pk: &G1, ciphersuite: u8) -> G2 {
        let pk_bytes = {
            let mut buf = [0u8; 48];
            let mut cur = Cursor::new(&mut buf[..]);
            assert!(pk.serialize(&mut cur, true).is_ok());
            buf
        };
        G2::core_sign(*sk, &pk_bytes[..], ciphersuite)
    }

    fn pop_verify(pk: G1, sig: G2, ciphersuite: u8) -> bool {
        let pk_bytes = {
            let mut buf = [0u8; 48];
            let mut cur = Cursor::new(&mut buf[..]);
            assert!(pk.serialize(&mut cur, true).is_ok());
            buf
        };
        G2::core_verify(pk, sig, &pk_bytes[..], ciphersuite)
    }
}
