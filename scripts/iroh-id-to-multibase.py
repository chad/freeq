#!/usr/bin/env python3
"""Convert an iroh endpoint ID (or secret key file) to a did:web publicKeyMultibase value.

Usage:
    python3 scripts/iroh-id-to-multibase.py <endpoint_id_hex>
    python3 scripts/iroh-id-to-multibase.py /path/to/iroh-key.secret

The endpoint ID is the ed25519 public key in hex. This script converts it
to multibase format (z + base58btc(0xed01 + raw_pubkey_bytes)) as required
by the did:web DID document spec (Multikey encoding).
"""

import sys
import os

# ed25519 multicodec prefix
ED25519_PREFIX = bytes([0xed, 0x01])

# Base58 alphabet (Bitcoin/IPFS variant)
B58_ALPHABET = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"

def b58encode(data: bytes) -> str:
    """Base58 encode (no external deps needed)."""
    n = int.from_bytes(data, "big")
    result = []
    while n > 0:
        n, r = divmod(n, 58)
        result.append(B58_ALPHABET[r:r+1])
    # Leading zero bytes → leading '1's
    for byte in data:
        if byte == 0:
            result.append(b"1")
        else:
            break
    return b"".join(reversed(result)).decode()

def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(1)

    arg = sys.argv[1]

    # If it's a file path, read the hex secret key and derive the public key
    if os.path.exists(arg):
        hex_str = open(arg).read().strip()
        if len(hex_str) == 64:
            # This is a secret key — we need the public key (endpoint ID)
            # The endpoint ID is printed by freeq-server on startup.
            # For ed25519, we can derive it, but that requires crypto libs.
            print(f"Secret key file detected ({arg})")
            print(f"Run freeq-server with --iroh and note the endpoint ID from the log:")
            print(f"  'Iroh ready. Connect with: --iroh-addr <ENDPOINT_ID>'")
            print(f"Then run: python3 {sys.argv[0]} <ENDPOINT_ID>")
            sys.exit(1)
        endpoint_id = hex_str
    else:
        endpoint_id = arg.strip()

    # Validate hex
    try:
        pub_bytes = bytes.fromhex(endpoint_id)
    except ValueError:
        print(f"Error: '{endpoint_id}' is not valid hex")
        sys.exit(1)

    if len(pub_bytes) != 32:
        print(f"Error: expected 32 bytes, got {len(pub_bytes)}")
        sys.exit(1)

    # Multicodec encode: 0xed01 prefix + raw public key
    multicodec = ED25519_PREFIX + pub_bytes
    multibase = "z" + b58encode(multicodec)

    print(f"Endpoint ID: {endpoint_id}")
    print(f"publicKeyMultibase: {multibase}")
    print()
    print("Add to your DID document's verificationMethod:")
    print(f'  "publicKeyMultibase": "{multibase}"')

if __name__ == "__main__":
    main()
