#!/usr/bin/env python3
"""Extract an ed25519 public key from an iroh secret key file or endpoint ID,
and encode it as a did:web publicKeyMultibase value.

Usage:
    python3 scripts/iroh-id-to-multibase.py /path/to/iroh-key.secret
    python3 scripts/iroh-id-to-multibase.py <endpoint_id_hex>

The endpoint ID is derived from the server's transport keypair. This script
extracts the ed25519 public key and encodes it as Multikey format:
    z + base58btc(0xed01 + 32_byte_pubkey)

When given a secret key file, derives the public key directly.
When given a hex string, treats it as the public key (endpoint ID).
"""

import sys
import os
import hashlib

# ed25519 multicodec prefix
ED25519_PREFIX = bytes([0xed, 0x01])

# Base58 alphabet (Bitcoin/IPFS variant)
B58_ALPHABET = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"


def b58encode(data: bytes) -> str:
    """Base58 encode (no external deps)."""
    n = int.from_bytes(data, "big")
    result = []
    while n > 0:
        n, r = divmod(n, 58)
        result.append(B58_ALPHABET[r : r + 1])
    for byte in data:
        if byte == 0:
            result.append(b"1")
        else:
            break
    return b"".join(reversed(result)).decode()


def ed25519_pubkey_from_secret(secret_bytes: bytes) -> bytes:
    """Derive ed25519 public key from 32-byte secret key.

    Uses the standard ed25519 derivation: SHA-512 the secret, clamp the
    lower 32 bytes, then scalar multiply by the base point.
    Requires no external crypto library — pure Python.
    """
    # We need actual ed25519 math for this. Rather than implement the full
    # curve in pure Python, we'll try to use available libraries.
    try:
        # Python 3.6+ on most systems has this via OpenSSL
        from cryptography.hazmat.primitives.asymmetric.ed25519 import (
            Ed25519PrivateKey,
        )
        from cryptography.hazmat.primitives.serialization import (
            Encoding,
            PublicFormat,
        )

        private_key = Ed25519PrivateKey.from_private_bytes(secret_bytes)
        pub_bytes = private_key.public_key().public_bytes(
            Encoding.Raw, PublicFormat.Raw
        )
        return pub_bytes
    except ImportError:
        pass

    try:
        # ed25519-dalek Python bindings or PyNaCl
        import nacl.signing

        signing_key = nacl.signing.SigningKey(secret_bytes)
        return bytes(signing_key.verify_key)
    except ImportError:
        pass

    return None


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    arg = sys.argv[1]

    pub_bytes = None

    if os.path.exists(arg):
        hex_str = open(arg).read().strip()
        if len(hex_str) != 64:
            print(f"Error: expected 64 hex chars in key file, got {len(hex_str)}")
            sys.exit(1)

        secret_bytes = bytes.fromhex(hex_str)
        pub_bytes = ed25519_pubkey_from_secret(secret_bytes)

        if pub_bytes is None:
            print(f"Secret key file detected ({arg})")
            print()
            print("To derive the public key, install one of:")
            print("  pip install cryptography")
            print("  pip install pynacl")
            print()
            print("Or use the endpoint ID printed by freeq-server at startup:")
            print(f"  python3 {sys.argv[0]} <ENDPOINT_ID>")
            sys.exit(1)

        print(f"Secret key file: {arg}")
    else:
        endpoint_id = arg.strip()
        try:
            pub_bytes = bytes.fromhex(endpoint_id)
        except ValueError:
            print(f"Error: '{endpoint_id}' is not valid hex")
            sys.exit(1)

        if len(pub_bytes) != 32:
            print(f"Error: expected 32 bytes (64 hex chars), got {len(pub_bytes)}")
            sys.exit(1)

        print(f"Public key (hex): {endpoint_id}")

    # Multicodec encode: 0xed01 prefix + raw public key
    multicodec = ED25519_PREFIX + pub_bytes
    multibase = "z" + b58encode(multicodec)

    print(f"publicKeyMultibase: {multibase}")
    print()
    print("DID document verificationMethod entries:")
    print()
    print('    {')
    print('      "id": "did:web:YOUR_DOMAIN#id-1",')
    print('      "type": "Multikey",')
    print('      "controller": "did:web:YOUR_DOMAIN",')
    print(f'      "publicKeyMultibase": "{multibase}"')
    print("    },")
    print("    {")
    print('      "id": "did:web:YOUR_DOMAIN#s2s-sig-1",')
    print('      "type": "Multikey",')
    print('      "controller": "did:web:YOUR_DOMAIN",')
    print(f'      "publicKeyMultibase": "{multibase}"')
    print("    }")
    print()
    print("(Both use the same key today. Split them later if needed.)")


if __name__ == "__main__":
    main()
