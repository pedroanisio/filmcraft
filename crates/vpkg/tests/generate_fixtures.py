"""Reproduce the tiny original oracle projects with sr-core's pack CLI.

Usage: python3 crates/vpkg/tests/generate_fixtures.py /path/to/sr-core
No Rust toolchain is invoked. Scratch projects live under target/vpkg-oracles.
"""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
core = Path(sys.argv[1]).resolve()
for name in ("minimal", "includes", "fetch"):
    project = root / "target/vpkg-oracles" / name
    project.mkdir(parents=True, exist_ok=True)
    spec = {
        "format": "scene-video-package", "formatVersion": "1.1",
        "package": {"id": name, "version": "1.0.0", "title": name, "created": "2026-10-14"},
        "scenes": [{"path": "main.scene", "role": "primary"}],
    }
    assets = ""
    if name == "includes":
        (project / "nested").mkdir(exist_ok=True)
        (project / "nested/assets.xml").write_text('<assets><data id="d" src="values.json"/></assets>')
        (project / "nested/values.json").write_text('{"value":7}\n')
        assets = '<include src="nested/assets.xml"/>'
    if name == "fetch":
        assets = '<assets><data id="d" src="download.json"/><image id="i" src="generated/frame.png"/></assets>'
        spec["fetch"] = [{"path": "download.json", "url": "https://example.org/download.json",
                          "sha256": hashlib.sha256(b"{}").hexdigest(), "size": 2}]
        spec["pipeline"] = {"steps": [{"id": "frames", "kind": "command", "run": ["generate"],
                                       "outputs": ["generated/*.png"]}]}
    (project / "main.scene").write_text('<scene version="1.7"><project width="320" height="180" '
                                      'fps="24" duration="1"/>' + assets + '<composition/></scene>')
    (project / "vpkg.json").write_text(json.dumps(spec))
    out = project / (name + ".vpkg.zip")
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    env.pop("SOURCE_DATE_EPOCH", None)
    subprocess.run(["python3", "-m", "sr_core.vpkg", "pack", str(project), "-o", str(out),
                    "--no-system-fonts"], cwd=core, env=env, check=True)
    assert out.stat().st_size < 20 * 1024
    shutil.copyfile(out, root / "crates/vpkg/tests/fixtures" / out.name)
