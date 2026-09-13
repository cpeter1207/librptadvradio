#!/usr/bin/env python3
"""Exercise launcher cleanup without a Docker daemon or image download."""

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path

LAUNCHER = Path(__file__).resolve().parents[1] / "tools/run-in-quality-container.sh"
DOCKER = r"""#!/usr/bin/env python3
import json
import os
from pathlib import Path
import sys

args = sys.argv[1:]
with Path(os.environ["DOCKER_CALL_LOG"]).open("a", encoding="utf-8") as log:
    log.write(json.dumps(args) + "\n")
if args[:2] == ["container", "ls"]:
    required = {
        "label=rpt_advanced.test=true",
        "label=org.rptadvanced.test.project=librptadvradio",
        "status=exited", "status=dead",
    }
    scoped = any(a.startswith("label=org.rptadvanced.test.scope=") for a in args)
    if not required.issubset(args) or not scoped:
        raise SystemExit(90)
    print("stopped-test\nrestarted-test")
elif args[:2] == ["container", "rm"]:
    if args[-1] == "restarted-test":
        # Docker refuses to remove a container which became active again.
        raise SystemExit(1)
elif args[:2] == ["image", "inspect"]:
    print("sha256:test-image")
elif args[:1] == ["run"]:
    raise SystemExit(int(os.environ["DOCKER_RUN_STATUS"]))
else:
    raise SystemExit(91)
"""


class LauncherCleanupTest(unittest.TestCase):
    """Keep sibling checks alive and clean our own container on every exit."""

    def run_launcher(self, status):
        """Run with a strict fake Docker and return its ordered invocations."""
        with tempfile.TemporaryDirectory(prefix="rptadvradio-launcher-") as directory:
            root = Path(directory)
            docker = root / "docker"
            docker.write_text(DOCKER, encoding="utf-8")
            docker.chmod(0o755)
            log = root / "calls.jsonl"
            env = dict(
                os.environ,
                PATH=f"{root}{os.pathsep}{os.environ['PATH']}",
                DOCKER_CALL_LOG=str(log),
                DOCKER_RUN_STATUS=str(status),
                RPTADV_CONTAINER_PULL="0",
            )
            result = subprocess.run(
                ["sh", str(LAUNCHER), "test-image:latest", "true"],
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(result.returncode, status, result.stderr)
            return [json.loads(line) for line in log.read_text().splitlines()]

    def test_only_stale_cleanup_is_non_forcing(self):
        """A discovery/removal race must not abort or kill concurrent work."""
        calls = self.run_launcher(0)
        removals = [call for call in calls if call[:2] == ["container", "rm"]]
        self.assertEqual(
            removals[:2],
            [
                ["container", "rm", "stopped-test"],
                ["container", "rm", "restarted-test"],
            ],
        )
        run = next(call for call in calls if call[:1] == ["run"])
        name = run[run.index("--name") + 1]
        self.assertEqual(removals[-1], ["container", "rm", "--force", name])
        self.assertIn("--rm", run)
        self.assertTrue(name.startswith("librptadvradio-test-"))

    def test_command_failure_still_cleans_own_container(self):
        """The EXIT trap must preserve the command's failure status."""
        calls = self.run_launcher(17)
        run = next(call for call in calls if call[:1] == ["run"])
        name = run[run.index("--name") + 1]
        self.assertEqual(calls[-1], ["container", "rm", "--force", name])


if __name__ == "__main__":
    unittest.main()
