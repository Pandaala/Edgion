#!/usr/bin/env python3
"""Check server-leaf roles, legacy cache replacement and trust-file consistency."""
import base64
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = Path('edgion-tests/integration/scripts/gen_certs/generate_tls_certs.sh')


class ServerLeafFixtureTests(unittest.TestCase):
    def run_command(self, *args):
        result = subprocess.run(args, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, timeout=60, check=True)
        return result.stdout

    def cert_text(self, path):
        return self.run_command('openssl', 'x509', '-in', str(path), '-noout', '-text')

    def assert_leaf(self, path):
        text = self.cert_text(path)
        self.assertRegex(text, r'Basic Constraints: critical\s+CA:FALSE')
        self.assertNotIn('CA:TRUE', text)

    def seed_legacy_ca(self, cert, key, subject):
        text = self.cert_text(cert)
        sans = re.search(r'Subject Alternative Name:[^\n]*\n\s*([^\n]+)', text).group(1).strip()
        self.run_command('openssl', 'req', '-new', '-x509', '-key', str(key),
                         '-out', str(cert), '-days', '1', '-subj', subject,
                         '-addext', 'basicConstraints=critical,CA:TRUE',
                         '-addext', 'subjectAltName=' + sans)
        self.assertIn('CA:TRUE', self.cert_text(cert))
        return cert.read_bytes()

    def test_server_profiles_replace_legacy_cache_and_reuse_valid_leaves(self):
        with tempfile.TemporaryDirectory(prefix='edgion-tls-leaf-') as directory:
            root = Path(directory)
            script = root / SCRIPT
            script.parent.mkdir(parents=True)
            shutil.copy2(ROOT / SCRIPT, script)
            certs = root / 'edgion-tests/certs'
            names = ['server.crt', 'server-ecdsa.crt', 'lkg-valid.crt']

            self.run_command('bash', str(script))
            for name in names:
                self.assert_leaf(certs / name)

            legacy_rsa = self.seed_legacy_ca(certs / 'server.crt', certs / 'server.key',
                                             '/CN=test.example.com')
            legacy_ecdsa = self.seed_legacy_ca(certs / 'server-ecdsa.crt', certs / 'server-ecdsa.key',
                                               '/CN=ecdsa-dualcert.example.com')
            (certs / 'ca.pem').write_bytes(legacy_rsa)
            self.run_command('bash', str(script))
            for name in names:
                self.assert_leaf(certs / name)
            self.assertNotEqual((certs / 'server.crt').read_bytes(), legacy_rsa)
            self.assertNotEqual((certs / 'server-ecdsa.crt').read_bytes(), legacy_ecdsa)
            self.assertEqual((certs / 'ca.pem').read_bytes(), (certs / 'server.crt').read_bytes())
            self.assertRegex(self.cert_text(certs / 'server.crt'), r'DNS:grpc\.example\.com(?=,|\s|$)')
            verify = ['openssl', 'verify', '-CAfile', str(certs / 'ca.pem')]
            help_text = subprocess.run(['openssl', 'verify', '-help'], stdout=subprocess.PIPE,
                                       stderr=subprocess.STDOUT, text=True, timeout=10).stdout
            # LibreSSL still verifies the chain and the exact required SAN;
            # OpenSSL additionally exposes a dedicated hostname-verification flag.
            if '-verify_hostname' in help_text:
                verify.extend(['-verify_hostname', 'grpc.example.com'])
            self.run_command(*verify, str(certs / 'server.crt'))

            stable = {name: (certs / name).read_bytes()
                      for name in ['server.crt', 'server.key', 'server-ecdsa.crt', 'server-ecdsa.key']}
            # Repairing the trust alias must not rotate otherwise valid leaves.
            (certs / 'ca.pem').write_bytes(legacy_rsa)
            self.run_command('bash', str(script))
            for name, expected in stable.items():
                self.assertEqual((certs / name).read_bytes(), expected)
            self.assertEqual((certs / 'ca.pem').read_bytes(), stable['server.crt'])
            secret = (root / 'tmp/integration_testing/generated_secrets_manual/base/Secret_edgion-test_edge-tls.yaml').read_text()
            encoded = re.search(r'^  tls.crt: (.+)$', secret, re.MULTILINE).group(1)
            self.assertEqual(base64.b64decode(encoded, validate=True), stable['server.crt'])


if __name__ == '__main__':
    unittest.main()
