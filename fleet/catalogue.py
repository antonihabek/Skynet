#!/usr/bin/env python3
"""Write `catalogue.json`: every description the sweep runs, pinned by commit and by bytes.

The descriptions come from three places, and each is pinned the same two ways the Apollo row is
(`drivers/apollo/run-check.sh`): the commit the file was taken at, then the sha256 of the file.

* **`robot_descriptions`**, at the version `requirements.txt` names. Its `_repositories.py` pins every
  upstream repository to a commit, and it fetches exactly that commit. A description that is a xacro
  template is pinned by its template's bytes; the URDF it renders to is the importer's input.
* **DeepMind's menagerie**, for the models `robot_descriptions` does not list — the same clone, so
  the same commit.
* **This repository's own vendor descriptions**, under `drivers/ros2_control/urdf/`, pinned by bytes
  only, because their commit is this one.

Regenerate when `requirements.txt` moves or `robots.py` gains a row; `tools/check-fleet.mjs` fails
when the two disagree, so a robot added to one and not the other is caught rather than silently
skipped.

    python3 fleet/catalogue.py
"""

from __future__ import annotations

import hashlib
import importlib
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, HERE)

import robots as robots_mod  # noqa: E402


def sha256_of(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> int:
    import robot_descriptions
    from robot_descriptions._descriptions import DESCRIPTIONS
    from robot_descriptions._repositories import REPOSITORIES

    cache = os.path.expanduser(os.environ.get("ROBOT_DESCRIPTIONS_CACHE", "~/.cache/robot_descriptions"))
    by_cache = {repo.cache_path: repo for repo in REPOSITORIES.values()}

    unlisted = sorted(set(DESCRIPTIONS) - set(robots_mod.ROBOTS))
    if unlisted:
        print(
            "catalogue: robot_descriptions lists descriptions robots.py does not: "
            + ", ".join(unlisted)
            + ". Give each a maker, a model and a class, or the sweep would run them unnamed.",
            file=sys.stderr,
        )
        return 1

    entries = []
    menagerie_root = importlib.import_module("robot_descriptions.panda_mj_description").REPOSITORY_PATH
    menagerie = by_cache[os.path.relpath(menagerie_root, cache).split(os.sep)[0]]
    for name, (maker, model, cls, route) in robots_mod.ROBOTS.items():
        if name.startswith("menagerie:"):
            rel = name.split(":", 1)[1]
            path = os.path.join(menagerie_root, rel)
            source = {
                "kind": "menagerie",
                "format": "mjcf",
                "repository": menagerie.url,
                "commit": menagerie.commit,
                "file": rel,
                "sha256": sha256_of(path),
            }
        elif name.startswith("skynet:"):
            rel = os.path.join("drivers", "ros2_control", "urdf", name.split(":", 1)[1])
            source = {"kind": "skynet", "format": "urdf", "file": rel, "sha256": sha256_of(os.path.join(REPO, rel))}
        else:
            module = importlib.import_module(f"robot_descriptions.{name}")
            root = module.REPOSITORY_PATH
            repository = by_cache[os.path.relpath(root, cache).split(os.sep)[0]]
            if hasattr(module, "MJCF_PATH"):
                fmt, path = "mjcf", module.MJCF_PATH
            elif hasattr(module, "URDF_PATH"):
                fmt, path = "urdf", module.URDF_PATH
            else:
                fmt, path = "xacro", module.XACRO_PATH
            source = {
                "kind": "robot_descriptions",
                "module": name,
                "format": fmt,
                "repository": repository.url,
                "commit": repository.commit,
                "file": os.path.relpath(path, root),
                "sha256": sha256_of(path),
            }
        entries.append(
            {"id": name, "maker": maker, "model": model, "class": cls, "route": route, "source": source}
        )
    entries.sort(key=lambda entry: entry["id"])
    out = {
        "note": (
            "Written by fleet/catalogue.py from fleet/robots.py. Every description fleet/sweep.py runs, pinned "
            "by upstream commit and by sha256. Regenerate rather than edit: tools/check-fleet.mjs holds this "
            "file to robots.py and to reference/reports/fleet.json."
        ),
        "robot_descriptions": robot_descriptions.__version__,
        "entries": entries,
    }
    with open(os.path.join(HERE, "catalogue.json"), "w", encoding="utf-8") as handle:
        json.dump(out, handle, indent=1, ensure_ascii=False)
        handle.write("\n")
    print(f"catalogue: {len(entries)} descriptions, {len({(e['maker'], e['model']) for e in entries})} robots")
    return 0


if __name__ == "__main__":
    sys.exit(main())
