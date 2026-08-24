"""jedimem test suite. Standard library only; no network, no LLM."""
import os
import pathlib
import subprocess
import sys
import tempfile
import unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent / "src"))

from jedimem import compiler, config, importers, repo          # noqa: E402
from jedimem.memory import Memory, MemoryError, ulid           # noqa: E402
from jedimem.redact import redact                              # noqa: E402
from jedimem.store import Store                                # noqa: E402

BIN = str(pathlib.Path(__file__).resolve().parent.parent / "bin" / "jedimem")


def git(*args, cwd):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=True)


def new_repo() -> pathlib.Path:
    d = pathlib.Path(tempfile.mkdtemp(prefix="jedimem-t-"))
    git("init", "-q", str(d), cwd=None) if False else subprocess.run(
        ["git", "init", "-q", str(d)], check=True, capture_output=True)
    git("config", "user.email", "t@example.com", cwd=d)
    git("config", "user.name", "Test", cwd=d)
    (d / "f.txt").write_text("base\n")
    git("add", "-A", cwd=d); git("commit", "-qm", "base", cwd=d)
    return d


class TestMemory(unittest.TestCase):
    def test_roundtrip(self):
        m = Memory.create("gotcha", "Point DATABASE_URL at the docker pg.\n\n**Why:** stale schema.",
                          scope="tests/**")
        r = Memory.from_text(m.to_text())
        self.assertEqual(r.id, m.id)
        self.assertEqual(r.kind, "gotcha")
        self.assertEqual(r.scope, "tests/**")
        self.assertEqual(r.body, m.body)
        r.validate(filename=r.id)

    def test_headline_is_first_paragraph_not_first_line(self):
        m = Memory.create("convention", "one\ntwo\n\nthree")
        self.assertEqual(m.headline, "one two")

    def test_ulids_unique_in_same_millisecond(self):
        ids = {ulid() for _ in range(500)}
        self.assertEqual(len(ids), 500)

    def test_short_handle_uses_random_half(self):
        # The first 10 chars are the timestamp; same-batch ids collide there.
        a, b = Memory.create("topic", "aaaa bbbb"), Memory.create("topic", "cccc dddd")
        self.assertEqual(a.id[:10], b.id[:10])          # documents the hazard
        self.assertNotEqual(a.short, b.short)           # ...and that we avoid it

    def test_identical_content_same_hash_across_machines(self):
        a = Memory.create("convention", "Use httpClient, not axios.")
        b = Memory.create("convention", "use   HTTPCLIENT, not axios.  ")
        self.assertEqual(a.content_hash, b.content_hash)

    def test_rejects_bad_records(self):
        for mutate in (lambda m: setattr(m, "kind", "nonsense"),
                       lambda m: setattr(m, "status", "nope"),
                       lambda m: setattr(m, "body", "  "),
                       lambda m: setattr(m, "format", 99)):
            m = Memory.create("convention", "something durable and long enough")
            mutate(m)
            with self.assertRaises(MemoryError):
                m.validate()

    def test_memory_cannot_grant_capability(self):
        m = Memory.create("convention", "something durable and long enough")
        m.extra["allowed_tools"] = "Bash"
        with self.assertRaises(MemoryError):
            m.validate()

    def test_memory_cannot_have_a_person_as_subject(self):
        m = Memory.create("convention", "something durable and long enough")
        m.extra["subject_person"] = "alice"
        with self.assertRaises(MemoryError):
            m.validate()

    def test_weaker_provenance_cannot_supersede_stronger(self):
        human = Memory.create("convention", "a rule a human confirmed", confirmed_by="human")
        agent = Memory.create("convention", "a rule an agent guessed", confirmed_by="agent")
        self.assertFalse(human.may_be_superseded_by(agent))
        self.assertTrue(agent.may_be_superseded_by(human))


class TestRedact(unittest.TestCase):
    def test_catches_common_shapes(self):
        for text in ("key sk-ant-api03-AAAAAAAAAAAAAAAAAAAAAAAA",
                     "token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345",
                     'export API_KEY="hunter2hunter2"',
                     "https://user:pa55word@example.com/x",
                     "aws AKIAIOSFODNN7EXAMPLE"):
            self.assertTrue(redact(text)[1], f"missed: {text}")

    def test_leaves_ordinary_prose_alone(self):
        t = "Use the internal httpClient wrapper, not axios, because of tracing."
        self.assertEqual(redact(t), (t, []))


class TestCompiler(unittest.TestCase):
    def _mems(self):
        return [Memory.create("requirement", "Migrations reviewed by platform.", status="active"),
                Memory.create("convention", "Use httpClient not axios.", scope="src/**",
                              status="active"),
                Memory.create("runbook", "Deploy with make ship.", status="active")]

    def test_tiers_are_separated(self):
        out = compiler.render(self._mems(), marker_begin="<!--B-->", marker_end="<!--E-->")
        self.assertIn("## Always", out)
        self.assertIn("## When touching matching files", out)
        self.assertIn("## Available on request", out)

    def test_inactive_not_delivered(self):
        m = Memory.create("requirement", "A superseded rule.", status="superseded")
        out = compiler.render([m], marker_begin="<!--B-->", marker_end="<!--E-->")
        self.assertNotIn("superseded rule", out)

    def test_budget_demotes_and_says_so(self):
        out = compiler.render(self._mems(), always_chars=10,
                              marker_begin="<!--B-->", marker_end="<!--E-->")
        self.assertIn("demoted", out)

    def test_splice_is_idempotent(self):
        sec = compiler.render(self._mems(), marker_begin="<!--B-->", marker_end="<!--E-->")
        once = compiler.splice("# Mine\n\nhand written\n", sec, "<!--B-->", "<!--E-->")
        twice = compiler.splice(once, sec, "<!--B-->", "<!--E-->")
        self.assertEqual(once, twice)

    def test_splice_idempotent_when_file_is_only_our_section(self):
        sec = compiler.render(self._mems(), marker_begin="<!--B-->", marker_end="<!--E-->")
        once = compiler.splice("", sec, "<!--B-->", "<!--E-->")
        twice = compiler.splice(once, sec, "<!--B-->", "<!--E-->")
        self.assertEqual(once, twice)

    def test_handwritten_content_survives(self):
        sec = compiler.render(self._mems(), marker_begin="<!--B-->", marker_end="<!--E-->")
        out = compiler.splice("# Mine\n\nkeep me\n", sec, "<!--B-->", "<!--E-->")
        self.assertIn("keep me", out)
        self.assertIn("# Mine", out)


class TestStagingRef(unittest.TestCase):
    def setUp(self):
        self.d = new_repo()

    def test_staging_never_dirties_the_worktree(self):
        store = Store(self.d)
        store.stage([Memory.create("gotcha", "Something worth remembering here.")])
        status = git("status", "--porcelain", cwd=self.d).stdout.strip()
        self.assertEqual(status, "", "staging must not touch the working tree")
        self.assertEqual(git("rev-list", "--count", "HEAD", cwd=self.d).stdout.strip(), "1")

    def test_pending_roundtrip_and_promote(self):
        store = Store(self.d)
        m = Memory.create("gotcha", "Something worth remembering here.")
        store.stage([m])
        pend = store.pending()
        self.assertEqual([p.id for p in pend], [m.id])
        store.promote([m.id])
        self.assertEqual(store.pending(), [])
        self.assertEqual([x.id for x in store.all()], [m.id])

    def test_concurrent_writers_lose_nothing(self):
        """The measured failure mode: porcelain commits dropped 13 of 20."""
        import threading
        store = Store(self.d)
        errs = []

        def w(i):
            try:
                store.stage([Memory.create("topic", f"Concurrent memory number {i} body.")])
            except Exception as e:      # pragma: no cover
                errs.append(e)

        ts = [threading.Thread(target=w, args=(i,)) for i in range(12)]
        [t.start() for t in ts]
        [t.join() for t in ts]
        self.assertEqual(errs, [])
        self.assertEqual(len(store.pending()), 12)


class TestImporters(unittest.TestCase):
    def setUp(self):
        self.d = new_repo()
        (self.d / "CLAUDE.md").write_text(
            "# Rules\n"
            "- Use the internal httpClient wrapper, never axios directly.\n"
            "- All migrations must be reviewed by the platform team first.\n")
        (self.d / ".claude" / "rules").mkdir(parents=True)
        (self.d / ".claude" / "rules" / "api.md").write_text(
            "# API\n- Handlers must validate input with the shared zod schemas.\n")
        (self.d / "docs" / "adr").mkdir(parents=True)
        (self.d / "docs" / "adr" / "0002-gql.md").write_text(
            "# Adopt GraphQL\n## Status\nSuperseded\n## Decision\n"
            "We added a GraphQL gateway to unify client access; it regressed latency badly.\n")
        (self.d / ".github").mkdir(exist_ok=True)
        (self.d / ".github" / "CODEOWNERS").write_text("/billing/  @acme/payments\n")

    def test_instructions_including_rule_directories(self):
        got = importers.from_instructions(self.d)
        bodies = " ".join(m.body for m in got)
        self.assertIn("httpClient", bodies)
        self.assertIn("zod", bodies, "must scan .claude/rules/ too")

    def test_kind_inference(self):
        kinds = {m.headline: m.kind for m in importers.from_instructions(self.d)}
        self.assertEqual(next(v for k, v in kinds.items() if "migrations" in k), "requirement")

    def test_superseded_adr_becomes_negative_knowledge(self):
        got = importers.from_adrs(self.d)
        self.assertTrue(got)
        self.assertEqual(got[0].kind, "negative")

    def test_codeowners_carries_scope(self):
        got = importers.from_codeowners(self.d)
        self.assertEqual(got[0].kind, "ownership")
        self.assertEqual(got[0].scope, "billing/")

    def test_reverts_become_negative_memories(self):
        (self.d / "f.txt").write_text("x\n")
        git("commit", "-qam", 'Revert "Move auth into the gateway"\n\nIt broke SSO refresh.',
            cwd=self.d)
        got = importers.from_git_history(self.d)
        self.assertTrue(got)
        self.assertEqual(got[0].kind, "negative")
        self.assertIn("SSO", got[0].body)

    def test_import_is_idempotent(self):
        first, _ = importers.run(self.d, sources=["instructions"])
        hashes = {m.content_hash: m for m in first}
        second, stats = importers.run(self.d, sources=["instructions"],
                                      existing_hashes=hashes)
        self.assertEqual(second, [], "re-import must add nothing")
        self.assertGreater(stats["instructions"]["duplicate"], 0)

    def test_never_reimports_our_own_generated_section(self):
        (self.d / "AGENTS.md").write_text(
            "# A\n<!-- BEGIN jedimem -->\n- **[convention]** Generated rule that must not return.\n"
            "<!-- END jedimem -->\n")
        got = importers.from_instructions(self.d)
        self.assertFalse([m for m in got if "must not return" in m.body])

    def test_secrets_are_redacted_before_staging(self):
        (self.d / "CONVENTIONS.md").write_text(
            "# C\n- Deploy with the token ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345 in CI.\n")
        mems, stats = importers.run(self.d, sources=["instructions"])
        joined = " ".join(m.body for m in mems)
        self.assertNotIn("ghp_ABCDEFGHIJKLMNOPQRSTUVWXYZ012345", joined)
        self.assertGreater(stats["instructions"]["redacted"], 0)


class TestConfig(unittest.TestCase):
    def test_repo_config_cannot_change_privacy_settings(self):
        d = new_repo()
        (d / ".jedimem").mkdir()
        (d / ".jedimem" / "config.yml").write_text(
            "repo_id: ABC\npaused: false\nruntime: api\nalways_chars: 1234\n")
        cfg = config.load(d)
        self.assertEqual(cfg["repo_id"], "ABC")
        self.assertEqual(cfg["always_chars"], 1234)
        # A cloned repo must not be able to set these on your machine.
        self.assertEqual(cfg["runtime"], "auto")

    def test_env_overrides(self):
        d = new_repo()
        os.environ["JEDIMEM_ALWAYS_CHARS"] = "42"
        try:
            self.assertEqual(config.load(d)["always_chars"], 42)
        finally:
            del os.environ["JEDIMEM_ALWAYS_CHARS"]


class TestCLI(unittest.TestCase):
    def run_cli(self, *args, cwd, expect=0):
        p = subprocess.run([BIN, *args], cwd=cwd, capture_output=True, text=True,
                           env={**os.environ, "NO_COLOR": "1"})
        self.assertEqual(p.returncode, expect,
                         f"{' '.join(args)} -> {p.returncode}\n{p.stdout}\n{p.stderr}")
        return p.stdout

    def test_full_workflow(self):
        d = new_repo()
        (d / "CLAUDE.md").write_text(
            "# Rules\n- Use the internal httpClient wrapper, never axios directly.\n")
        self.run_cli("init", cwd=d)
        self.assertIn("candidate", self.run_cli("import", cwd=d))
        self.run_cli("import", "--stage", cwd=d)
        out = self.run_cli("review", cwd=d)
        handle = out.split("\n")[2].split()[0]
        self.run_cli("review", "--approve", handle, cwd=d)
        self.run_cli("compile", "--check", cwd=d)                 # must be fresh
        self.assertIn("active", self.run_cli("status", cwd=d))
        self.assertIn("provenance", self.run_cli("why", "httpClient", cwd=d))
        self.run_cli("lint", cwd=d)
        self.assertEqual(git("status", "--porcelain", cwd=d).stdout.count("CLAUDE.md"), 1)

    def test_outside_a_git_repo_fails_clearly(self):
        d = pathlib.Path(tempfile.mkdtemp())
        p = subprocess.run([BIN, "status"], cwd=d, capture_output=True, text=True)
        self.assertNotEqual(p.returncode, 0)
        self.assertIn("git", (p.stdout + p.stderr).lower())


if __name__ == "__main__":
    unittest.main(verbosity=2)
