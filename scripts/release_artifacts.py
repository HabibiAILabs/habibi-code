#!/usr/bin/env python3
"""Select exact-commit trusted CI artifacts and validate release archives. No rebuilds."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
import tomllib

TARGETS = (
    "x86_64-unknown-linux-musl", "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin", "aarch64-apple-darwin",
)


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_tag(tag, manifest):
    version = tomllib.loads(manifest)["workspace"]["package"]["version"]
    require(re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag) and tag == f"v{version}",
            "Stable release tag must match the workspace version")


def trusted_run(run, repo, sha, workflow_id):
    return (
        run.get("head_sha") == sha
        and run.get("workflow_id") == workflow_id
        and run.get("path") == ".github/workflows/ci.yml"
        and run.get("status") == "completed"
        and run.get("conclusion") == "success"
        and run.get("event") in ("push", "workflow_dispatch")
        and run.get("head_branch") == "main"
        and run.get("repository", {}).get("full_name", "").lower() == repo.lower()
        and run.get("head_repository", {}).get("full_name", "").lower() == repo.lower()
    )


def artifact_ids(artifacts, sha):
    expected = {f"habibi-{target}" for target in TARGETS}
    selected = [a for a in artifacts if a["name"] in expected]
    require(len(selected) == len(expected) and {a["name"] for a in selected} == expected,
            "CI artifacts are missing or duplicated; rebuild CI for the tagged commit")
    for artifact in selected:
        require(not artifact["expired"] and artifact["size_in_bytes"] > 0,
                "CI artifacts expired or empty; rebuild CI for the tagged commit")
        require(artifact.get("workflow_run", {}).get("head_sha") == sha,
                "Artifact commit does not match the release commit")
    return ",".join(str(int(a["id"])) for a in sorted(selected, key=lambda a: a["name"]))


def api(endpoint, paginate=False):
    args = ["gh", "api", endpoint]
    if paginate:
        args.extend(["--paginate", "--slurp"])
    return json.loads(subprocess.check_output(args, text=True))


def select_run(repo, sha):
    workflow = api(f"repos/{repo}/actions/workflows/ci.yml")
    pages = api(f"repos/{repo}/actions/workflows/{workflow['id']}/runs"
                f"?head_sha={sha}&status=success&branch=main&per_page=100", paginate=True)
    runs = [run for page in pages for run in page["workflow_runs"]
            if trusted_run(run, repo, sha, workflow["id"])]
    for run in sorted(runs, key=lambda run: run["id"], reverse=True):
        pages = api(f"repos/{repo}/actions/runs/{run['id']}/artifacts?per_page=100", paginate=True)
        try:
            ids = artifact_ids([a for page in pages for a in page["artifacts"]], sha)
        except ValueError:
            continue
        return run["id"], ids
    raise ValueError("No successful trusted main CI run with all four unexpired artifacts for "
                     f"{sha}. Run CI for this exact commit, then rerun Release; see docs/releasing.md")


def validate_archives(directory, tag):
    expected = {f"habibi-{tag}-{target}.tar.gz" for target in TARGETS}
    require({p.name for p in directory.iterdir()} == expected,
            "Release directory must contain exactly the four versioned platform archives")
    for name in sorted(expected):
        path = directory / name
        require(path.is_file() and not path.is_symlink() and path.stat().st_size > 0,
                "Invalid release archive")
        with tarfile.open(path, "r:gz") as archive:
            members = archive.getmembers()
            require(len(members) == 3 and {m.name for m in members} == {
                "habibi", "README.md", "docs/pi-secure-LICENSE"}, "Unexpected archive contents")
            require(all(m.isfile() and m.size > 0 for m in members),
                    "Archive contains empty files or non-regular entries")
            require(archive.getmember("habibi").mode & 0o111, "Binary is not executable")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("select", "verify"))
    parser.add_argument("--tag", required=True)
    parser.add_argument("--directory", type=Path, default=Path("dist"))
    args = parser.parse_args()
    validate_tag(args.tag, Path("Cargo.toml").read_text())
    if args.mode == "verify":
        validate_archives(args.directory, args.tag)
        print("Verified all four release archives")
        return
    repo = os.environ["GITHUB_REPOSITORY"]
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    run_id, ids = select_run(repo, sha)
    with open(os.environ["GITHUB_OUTPUT"], "a") as output:
        output.write(f"run_id={run_id}\nartifact_ids={ids}\n")
    print(f"Using trusted CI run {run_id} for commit {sha}")


if __name__ == "__main__":
    main()
