#!/usr/bin/env python3
"""Generate the synthetic native-smoke certificate using portable OpenSSL config."""
import pathlib
import subprocess
import sys

cert = pathlib.Path(sys.argv[1])
key = cert.with_suffix('.key')
config = cert.with_suffix('.cnf')
config.write_text(
    '[req]\nprompt = no\ndistinguished_name = dn\nx509_extensions = extensions\n'
    '[dn]\nCN = chatgpt.com\n[extensions]\n'
    'subjectAltName = DNS:chatgpt.com,DNS:localhost,IP:127.0.0.1\n'
)
subprocess.run(
    ['openssl', 'req', '-x509', '-newkey', 'rsa:2048', '-nodes', '-days', '2',
     '-keyout', str(key), '-out', str(cert), '-config', str(config)],
    check=True, stdout=subprocess.DEVNULL,
)
