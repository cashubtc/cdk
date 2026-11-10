"""Cross-file contracts protecting the existing supported package targets."""

from pathlib import Path
import re
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

import model


class InputTests(unittest.TestCase):
    def setUp(self):
        self.root = Path(__file__).resolve().parents[3]

    def test_every_dart_hook_target_has_a_candidate_package(self):
        hook = (self.root / "bindings/dart/hook/target.dart").read_text()
        advertised = set(re.findall(r"'([a-z0-9_]+-[a-z0-9_]+-[a-z0-9-]+)'", hook))
        self.assertEqual(advertised, set(model.DART_TARGETS))

    def test_gradle_wrapper_agrees_with_release_tool_and_hash(self):
        config = model.inputs(self.root)["gradle"]
        wrapper = (
            self.root / "bindings/kotlin/gradle/wrapper/gradle-wrapper.properties"
        ).read_text()
        self.assertIn(f"gradle-{config['version']}-bin.zip", wrapper)
        self.assertIn(f"distributionSha256Sum={config['sha256']}", wrapper)

    def test_supported_android_api_matches_package_manifest(self):
        config = model.inputs(self.root)["android"]
        project = (
            self.root / "bindings/kotlin/cdk-android/build.gradle.kts"
        ).read_text()
        self.assertRegex(project, rf"minSdk\s*=\s*{config['api']}\b")
        self.assertRegex(project, rf"compileSdk\s*=\s*{config['sdk']}\b")


if __name__ == "__main__":
    unittest.main()
