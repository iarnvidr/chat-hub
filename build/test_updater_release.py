import json, unittest, sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).parent))
from updater_release import stable, make_feed, validate_feed, verify_source_version

class UpdaterReleaseTests(unittest.TestCase):
    def test_stable_only(self):
        self.assertTrue(stable('1.2.3'))
        for x in ('1.2.3-preview.1','v1.2.3','1.2','1.2.03','1.2.3+meta'): self.assertFalse(stable(x), x)
    def test_bundle_version_matches_requested(self):
        verify_source_version("1.2.3", "1.2.3")
        for bundle, requested in (("1.2.4", "1.2.3"), ("1.2.3-beta", "1.2.3"), ("1.2.3", "1.2.3-beta")):
            with self.assertRaises(ValueError): verify_source_version(bundle, requested)
    def test_valid_feed(self):
        f=make_feed('1.2.3','https://github.com/o/r/releases/download/v1.2.3/Chat Hub.app.tar.gz','signed')
        self.assertEqual(f['platforms']['darwin-aarch64']['url'].split('/')[-1], 'Chat Hub.app.tar.gz')
        validate_feed(f)
    def test_malformed_schema(self):
        for f in ({}, {'platforms':[]}, {'version':'1.2.3','notes':'x','platforms':{'darwin-aarch64':{'url':'http://x/Chat Hub.app.tar.gz','signature':'s'}}}, {'version':'1.2.3','notes':'x','platforms':{'darwin-aarch64':{'url':'https://x/Chat Hub.app.tar.gz'}}}):
            with self.assertRaises(ValueError): validate_feed(f)
    def test_path_and_signature_requirements(self):
        with self.assertRaises(ValueError): make_feed('1.2.3','file:///tmp/app.tar.gz','sig')
        with self.assertRaises(ValueError): make_feed('1.2.3','https://example/app.tar.gz','')
        with self.assertRaises(ValueError): make_feed('1.2.3-preview.1','https://example/app.tar.gz','sig')
if __name__=='__main__': unittest.main()
