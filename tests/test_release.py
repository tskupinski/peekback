import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]


class ReleaseTests(unittest.TestCase):
    def test_all_hook_commands_work_with_spaces_in_install_path(self):
        with tempfile.TemporaryDirectory(prefix="peekback hook test ") as temporary:
            script = Path(temporary) / "hooks/register.sh"
            script.parent.mkdir()
            script.write_text('#!/bin/bash\nprintf "%s" "${1:-claude}"\n')
            script.chmod(0o755)
            for name, agent in [("settings-snippet.json", "claude"), ("codex-snippet.json", "codex")]:
                hooks = json.loads((ROOT / "hooks" / name).read_text())["hooks"]
                for event, entries in hooks.items():
                    for entry in entries:
                        for hook in entry["hooks"]:
                            command = hook["command"].replace("/absolute/path/to/peekback", temporary)
                            result = subprocess.check_output(["bash", "-c", command], text=True)
                            self.assertEqual(result, agent, event)

    def test_license_verification_rejects_changed_assets_and_missing_notices(self):
        spec = importlib.util.spec_from_file_location("vendor_licenses", ROOT / "scripts/vendor-licenses.py")
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        with tempfile.TemporaryDirectory() as temporary:
            module.ROOT = Path(temporary)
            module.DEST = module.ROOT / "licenses/frontend"
            module.DEST.mkdir(parents=True)
            vendor = module.ROOT / "assets/vendor"
            vendor.mkdir(parents=True)
            (vendor / "VERSIONS").write_text("example 1.0.0\n")
            asset = vendor / "example.js"
            asset.write_text("original")
            notice = module.DEST / "LICENSE"
            notice.write_text("notice")
            manifest = {"roots": {"example": "1.0.0"}, "assets": {
                str(p.relative_to(module.ROOT)): module.sha(p.read_bytes()) for p in vendor.iterdir()
            }, "packages": [{"files": {"LICENSE": module.sha(notice.read_bytes())}}]}
            (module.DEST / "manifest.json").write_text(json.dumps(manifest))
            module.check()
            asset.write_text("changed")
            with self.assertRaises(ValueError):
                module.check()
            asset.write_text("original")
            notice.unlink()
            with self.assertRaises(FileNotFoundError):
                module.check()


if __name__ == "__main__":
    unittest.main()
