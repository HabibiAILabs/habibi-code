#!/usr/bin/env python3
"""Hermetic release provenance, selection, and archive validation tests."""
import copy
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

import release_artifacts as release

REPO = "HabibiAILabs/habibi-code"
SHA = "a" * 40
RUN = {
    "id": 10, "workflow_id": 20, "path": ".github/workflows/ci.yml",
    "head_sha": SHA, "status": "completed", "conclusion": "success",
    "event": "push", "head_branch": "main",
    "repository": {"full_name": REPO}, "head_repository": {"full_name": REPO},
}
ARTIFACTS = [
    {"id": i, "name": f"habibi-{target}", "expired": False, "size_in_bytes": 100,
     "workflow_run": {"head_sha": SHA}}
    for i, target in enumerate(release.TARGETS, start=1)
]


class ReleaseTests(unittest.TestCase):
    def test_tag_must_be_stable_and_match_manifest(self):
        manifest = '[workspace.package]\nversion = "1.2.3"\n'
        release.validate_tag("v1.2.3", manifest)
        for tag in ("v1.2.4", "v1.2.3-rc1", "1.2.3", "v1.2.3\n"):
            with self.subTest(tag=tag), self.assertRaises(ValueError):
                release.validate_tag(tag, manifest)

    def test_only_trusted_successful_exact_commit_main_runs(self):
        self.assertTrue(release.trusted_run(RUN, REPO, SHA, 20))
        for key, value in (
            ("head_sha", "b" * 40), ("workflow_id", 21),
            ("path", ".github/workflows/other.yml"), ("status", "in_progress"),
            ("conclusion", "failure"), ("event", "pull_request"),
            ("event", "pull_request_target"), ("head_branch", "feature"),
            ("repository", {"full_name": "attacker/repo"}),
            ("head_repository", {"full_name": "attacker/repo"}),
        ):
            with self.subTest(key=key):
                self.assertFalse(release.trusted_run({**RUN, key: value}, REPO, SHA, 20))
        self.assertTrue(release.trusted_run({**RUN, "event": "workflow_dispatch"}, REPO, SHA, 20))

    def test_artifacts_must_be_complete_unexpired_and_commit_bound(self):
        self.assertEqual(set(release.artifact_ids(ARTIFACTS, SHA).split(",")), {"1", "2", "3", "4"})
        cases = [ARTIFACTS[:-1], ARTIFACTS + [ARTIFACTS[0]]]
        for key, value in (("expired", True), ("size_in_bytes", 0),
                           ("workflow_run", {"head_sha": "b" * 40})):
            case = copy.deepcopy(ARTIFACTS)
            case[0][key] = value
            cases.append(case)
        for case in cases:
            with self.subTest(case=case), self.assertRaises(ValueError):
                release.artifact_ids(case, SHA)

    def test_selection_uses_complete_exact_commit_run_not_latest(self):
        expired = copy.deepcopy(ARTIFACTS)
        expired[0]["expired"] = True
        responses = [
            {"id": 20},
            [{"workflow_runs": [{**RUN, "id": 12, "head_sha": "b" * 40}, {**RUN, "id": 11}]},
             {"workflow_runs": [RUN]}],
            [{"artifacts": expired}],
            [{"artifacts": ARTIFACTS[:2]}, {"artifacts": ARTIFACTS[2:]}],
        ]
        with patch.object(release, "api", side_effect=responses) as api:
            run_id, _ = release.select_run(REPO, SHA)
        self.assertEqual(run_id, 10)
        self.assertIn("head_sha=" + SHA, api.call_args_list[1].args[0])
        self.assertIn("/runs/11/artifacts", api.call_args_list[2].args[0])
        self.assertIn("/runs/10/artifacts", api.call_args_list[3].args[0])
        with patch.object(release, "api", side_effect=[{"id": 20}, [{"workflow_runs": []}]]):
            with self.assertRaisesRegex(ValueError, "No successful trusted"):
                release.select_run(REPO, SHA)

    def test_archive_validation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)

            def write_archive(path, bad_member=None):
                with tarfile.open(path, "w:gz") as archive:
                    for name in ("habibi", "README.md", "docs/pi-secure-LICENSE"):
                        info = tarfile.TarInfo(name)
                        info.mode = 0o755 if name == "habibi" else 0o644
                        info.size = 3
                        archive.addfile(info, io.BytesIO(b"abc"))
                    if bad_member:
                        archive.addfile(tarfile.TarInfo(bad_member))

            for target in release.TARGETS:
                write_archive(root / f"habibi-v1.2.3-{target}.tar.gz")
            release.validate_archives(root, "v1.2.3")
            with self.assertRaises(ValueError):
                release.validate_archives(root, "v1.2.4")
            (root / "unexpected").touch()
            with self.assertRaises(ValueError):
                release.validate_archives(root, "v1.2.3")
            (root / "unexpected").unlink()
            archive = next(root.iterdir())
            write_archive(archive, "../../unexpected")
            with self.assertRaises(ValueError):
                release.validate_archives(root, "v1.2.3")
            # Reject symlink entries even when their names look legitimate.
            with tarfile.open(archive, "w:gz") as tar:
                for name in ("habibi", "README.md", "docs/pi-secure-LICENSE"):
                    info = tarfile.TarInfo(name)
                    info.type = tarfile.SYMTYPE
                    info.linkname = "/etc/passwd"
                    tar.addfile(info)
            with self.assertRaises(ValueError):
                release.validate_archives(root, "v1.2.3")
            archive.unlink()
            with self.assertRaises(ValueError):
                release.validate_archives(root, "v1.2.3")


if __name__ == "__main__":
    unittest.main()
