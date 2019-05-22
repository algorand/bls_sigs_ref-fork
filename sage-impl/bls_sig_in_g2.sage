#!/usr/bin/env sage
# vim: syntax=python
#
# (C) Zhenfei Zhang <zhenfei@algorand.com>
# some modification and tidying by Riad S. Wahby <rsw@cs.stanford.edu>

from hashlib import sha256

from hash_to_field import hash_to_field
from util import print_iv, print_value

load("opt_sswu_g2.sage")
load("bls_sig_consts.sage")

# keygen takes in sk as byte[32] and outputs the secret exponent and the public key in G1
def keygen(sk, output_pk=True):
    # https://github.com/pairingwg/bls_standard/blob/master/minutes/spec-v1.md#basic-signature-in-g2
    (x_prime,) = hash_to_field(sk, 0, q, 1, sha256, 2)
    print_iv(x_prime, "x'", "keygen", False)
    return (x_prime, (x_prime * g1gen) if output_pk else None)

# signing algorithm as in
#     https://github.com/pairingwg/bls_standard/blob/master/minutes/spec-v1.md#basic-signature-in-g2
# sign takes in sk as byte[32], a message, and a ciphersuite id
# returns a signature in G2
def sign(sk, msg, ciphersuite):
    print_iv(msg, "input msg", "sign", True)

    # get the secret value x' by running keygen
    (x_prime, _) = keygen(sk, False)

    # hash the concatenation of the (one-byte) ciphersuite and the message
    msg_to_hash = "%c%s" % (ciphersuite, msg)
    print_iv(msg_to_hash, "msg to hash", "sign", False)

    # hash to the curve
    P = map2curve_osswu2(msg_to_hash)
    print_iv_g2(P, "hash to E2", "sign")

    # output the signature x' * P
    return x_prime * P

if __name__ == "__main__":
    # parameters for this signature
    ciphersuite = 2
    msg = "the message to be signed"
    sk =  "11223344556677889900112233445566"

    # generate the keys and the signature
    (_, pk) = keygen(sk, True)
    sig = sign(sk, msg, ciphersuite)

    # output the test vectors
    print "\n" * 3,
    print "================== start of test vectors ===================="
    print "==================    signature in G2    ===================="

    print "g1 generator:"
    print_g1_hex(g1gen)

    print "g2 generator:"
    print_g2_hex(g2gen)

    # XXX(rsw) do we need this?
    #print "g2 generator, IETF encoding:"
    #print_g2_hex_ieft(g2gen)

    print "group order: 0x%x" % q
    print "ciphersuite: 0x%x" % ciphersuite
    print "message:    ",
    print_value(msg, True)

    print "sk:         ",
    print_value(sk, True)

    print "public key:  "
    print_g1_hex(pk)

    print "signature:   "
    print_g2_hex(sig)

    print "==================  end of test vectors  ===================="