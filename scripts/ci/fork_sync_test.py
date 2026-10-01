"""Exercise the daily fork sync against real disposable Git repositories."""

import os
import subprocess
import tempfile
import textwrap
import unittest
from pathlib import Path

WORKFLOW = (
    Path(__file__).resolve().parents[2] / ".github/workflows/daily-master-sync.yml"
)


def git(root, *args):
    return subprocess.check_output(
        ["git", "-C", str(root), *args], text=True, stderr=subprocess.DEVNULL
    ).strip()


class ForkSyncTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.seed = self.root / "seed"
        self.seed.mkdir()
        git(self.seed, "init", "-b", "master")
        git(self.seed, "config", "user.name", "Test")
        git(self.seed, "config", "user.email", "test@example.invalid")
        self.commit(
            self.seed, "Cargo.toml", '[package]\nname="test"\nversion="0.1.0"\n'
        )
        for name in ("origin", "canonical", "upstream"):
            git(self.root, "clone", "--bare", str(self.seed), str(self.root / name))
        self.work = self.root / "work"
        git(self.root, "clone", str(self.root / "origin"), str(self.work))

    def commit(self, repo, name, content):
        (repo / name).write_text(content)
        git(repo, "add", name)
        git(
            repo,
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            name,
        )
        return git(repo, "rev-parse", "HEAD")

    def upstream_change(self, name, content):
        sha = self.commit(self.seed, name, content)
        git(self.seed, "push", str(self.root / "upstream"), "master")
        return sha

    def sync(self):
        body = textwrap.dedent(WORKFLOW.read_text().split("        run: |\n", 1)[1])
        body = body.replace(
            "https://github.com/zeroclaw-labs/zeroclaw.git", str(self.root / "upstream")
        )
        body = body.replace(
            "https://gitlab.com/ncz-os/zeroclaw.git", str(self.root / "canonical")
        )
        return subprocess.run(
            ["bash", "-c", body],
            cwd=self.work,
            env={**os.environ, "GITLAB_SYNC_TOKEN": "test-only"},
            text=True,
            capture_output=True,
            check=False,
        )

    def test_both_forges_advance_and_repeat_succeeds(self):
        sha = self.upstream_change("upstream.txt", "latest")
        result = self.sync()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("origin", "canonical"):
            self.assertEqual(git(self.root / name, "rev-parse", "master"), sha)
        git(self.work, "remote", "remove", "upstream")
        git(self.work, "remote", "remove", "canonical")
        self.assertEqual(self.sync().returncode, 0)

    def test_canonical_only_commit_is_preserved(self):
        self.commit(self.seed, "security.txt", "fix")
        git(self.seed, "push", str(self.root / "canonical"), "master")
        result = self.sync()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            git(self.root / "origin", "show", "master:security.txt"), "fix"
        )
        self.assertEqual(
            git(self.root / "origin", "rev-parse", "master"),
            git(self.root / "canonical", "rev-parse", "master"),
        )

    def test_invalid_manifest_never_advances_forges(self):
        before = git(self.root / "origin", "rev-parse", "master")
        self.upstream_change("Cargo.toml", '[package]\nname="one"\nname="duplicate"\n')
        self.assertNotEqual(self.sync().returncode, 0)
        for name in ("origin", "canonical"):
            self.assertEqual(git(self.root / name, "rev-parse", "master"), before)


if __name__ == "__main__":
    unittest.main()
