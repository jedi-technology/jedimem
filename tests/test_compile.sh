#!/bin/sh
# Contract tests for the compiler. No network, no LLM, no git writes.
set -u
cd "$(dirname "$0")/.." || exit 1
fails=0
t() { if [ "$2" = "$3" ]; then echo "  ok   $1"; else echo "  FAIL $1: expected '$3', got '$2'"; fails=$((fails+1)); fi }

# Work on a scratch copy so the test never mutates the repo.
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
cp -r .jedimem bin AGENTS.md CLAUDE.md "$tmp"/ 2>/dev/null
cd "$tmp" || exit 1

echo "compiler contract:"

./bin/jedimem-compile >/dev/null 2>&1
t "clean compile succeeds" "$?" "0"

./bin/jedimem-compile --check >/dev/null 2>&1
t "check passes when fresh" "$?" "0"

a=$(sha256sum AGENTS.md | cut -d' ' -f1)
./bin/jedimem-compile >/dev/null 2>&1
b=$(sha256sum AGENTS.md | cut -d' ' -f1)
t "output is byte-stable" "$a" "$b"

t "hand-written content preserved" "$(grep -c 'never touched by jedimem' AGENTS.md)" "1"

# Adding a memory must make the check fail — otherwise CI can't catch drift.
sed 's/^id: .*/id: 01JTESTTESTTESTTESTTESTTEST/' .jedimem/memories/*GITLOCK.md > .jedimem/memories/zz-test.md
./bin/jedimem-compile --check >/dev/null 2>&1
t "check FAILS when stale" "$?" "1"
rm -f .jedimem/memories/zz-test.md

# A superseded memory must not be delivered.
f=$(ls .jedimem/memories/*GITLOCK.md)
sed -i.bak 's/^status: active/status: superseded/' "$f" 2>/dev/null || sed -i '' 's/^status: active/status: superseded/' "$f"
./bin/jedimem-compile >/dev/null 2>&1
t "superseded memory is not delivered" "$(grep -c 'index.lock is a mutex' AGENTS.md)" "0"

# Malformed frontmatter must fail loudly, not silently drop a memory.
printf 'no frontmatter here\n' > .jedimem/memories/bad.md
./bin/jedimem-compile >/dev/null 2>&1
t "malformed memory fails loudly" "$?" "1"

if [ "$fails" -eq 0 ]; then echo "PASS: compiler contract holds"; exit 0; fi
echo "FAILURES: $fails"; exit 1
