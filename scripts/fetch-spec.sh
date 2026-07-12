#!/usr/bin/env bash
#
# Fetch the official Control D OpenAPI spec from their published documentation.
#
# Control D does not offer the spec as a download. However, every rendered
# API reference page embeds the *complete* spec in its page payload under
# `data.api.schema`. We fetch it from there.
#
# As a correctness check we pull all endpoint pages and assert that every one
# embeds a byte-identical spec, which guards against a partially-stale page.
#
# The Control D API is explicitly unversioned ("breaking changes can be
# introduced without warning"), so re-run this periodically and diff the result.
#
# Usage: ./scripts/fetch-spec.sh [output-path]

set -euo pipefail

BASE="https://docs.controld.com/controld/api-next/v2/branches/1.0"
OUT="${1:-docs/reference/controld-openapi.json}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "==> Fetching reference sidebar"
curl -fsSL "$BASE/sidebar?page_type=reference" -o "$TMP/sidebar.json"

python3 - "$TMP/sidebar.json" > "$TMP/slugs.txt" <<'PY'
import json, re, sys
data = json.load(open(sys.argv[1]))
slugs = []
def walk(nodes):
    for n in nodes:
        if n.get("type") == "endpoint":
            slug = n["slug"]
            # Slugs land in curl URLs and -o file paths; a hostile sidebar must
            # not be able to smuggle "../" or separators into either.
            if not re.fullmatch(r"[A-Za-z0-9_-]+", slug):
                sys.exit(f"FAIL: refusing suspicious slug {slug!r} from sidebar")
            slugs.append(slug)
        for key in ("pages", "children"):
            if n.get(key):
                walk(n[key])
walk(data if isinstance(data, list) else data.get("data", []))
print("\n".join(slugs))
PY

count=$(wc -l < "$TMP/slugs.txt")
echo "==> Found $count endpoint pages"

echo "==> Fetching all endpoint pages"
mkdir -p "$TMP/pages"
xargs -P 8 -I{} curl -fsSL "$BASE/reference/{}?reduce=false" -o "$TMP/pages/{}.json" < "$TMP/slugs.txt"

echo "==> Extracting and cross-verifying embedded spec"
python3 - "$TMP/pages" "$OUT" <<'PY'
import glob, hashlib, json, os, sys

pages_dir, out_path = sys.argv[1], sys.argv[2]
by_hash, specs = {}, {}

for path in sorted(glob.glob(os.path.join(pages_dir, "*.json"))):
    api = json.load(open(path))["data"]["api"]
    schema = api["schema"]
    digest = hashlib.sha256(json.dumps(schema, sort_keys=True).encode()).hexdigest()
    by_hash.setdefault(digest, []).append(os.path.basename(path))
    specs[digest] = schema

if len(by_hash) != 1:
    print(f"FAIL: pages disagree — {len(by_hash)} distinct specs found:", file=sys.stderr)
    for digest, pages in by_hash.items():
        print(f"  {digest[:16]}: {len(pages)} pages e.g. {pages[:3]}", file=sys.stderr)
    sys.exit(1)

digest, spec = next(iter(specs.items()))
ops = sum(
    1
    for item in spec["paths"].values()
    for method in item
    if method in ("get", "post", "put", "delete", "patch")
)

os.makedirs(os.path.dirname(out_path), exist_ok=True)
with open(out_path, "w") as fh:
    json.dump(spec, fh, indent=2)
    fh.write("\n")

pages = len(next(iter(by_hash.values())))
print(f"    all {pages} pages agree (sha256 {digest[:16]})")
print(f"    {len(spec['paths'])} paths, {ops} operations")
print(f"==> Wrote {out_path}")
PY
