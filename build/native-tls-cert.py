#!/usr/bin/env python3
"""Generate a strict-verification CA and signed TLS server leaf."""
import pathlib, subprocess, sys
ca = pathlib.Path(sys.argv[1]); stem = ca.with_suffix('')
leaf=stem.with_name(stem.name+'-leaf.pem'); key=stem.with_name(stem.name+'-leaf.key'); cakey=stem.with_name(stem.name+'-ca.key'); cfg=stem.with_suffix('.cnf')
cfg.write_text('''[req]\nprompt=no\ndistinguished_name=dn\n[dn]\nCN=Chat Hub synthetic fixture CA\n[ca_ext]\nbasicConstraints=critical,CA:true\nkeyUsage=critical,keyCertSign,cRLSign\nsubjectKeyIdentifier=hash\n[leaf_ext]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:chatgpt.com,DNS:localhost,IP:127.0.0.1\nsubjectKeyIdentifier=hash\nauthorityKeyIdentifier=keyid:always\n''')
def run(args): subprocess.run(args,check=True,stdout=subprocess.DEVNULL)
run(['openssl','req','-x509','-newkey','rsa:2048','-nodes','-days','3650','-keyout',str(cakey),'-out',str(ca),'-config',str(cfg),'-extensions','ca_ext'])
csr=stem.with_suffix('.csr')
run(['openssl','req','-new','-newkey','rsa:2048','-nodes','-keyout',str(key),'-out',str(csr),'-subj','/CN=chatgpt.com'])
run(['openssl','x509','-req','-in',str(csr),'-CA',str(ca),'-CAkey',str(cakey),'-CAcreateserial','-days','397','-out',str(leaf),'-extfile',str(cfg),'-extensions','leaf_ext'])
csr.unlink(missing_ok=True)
