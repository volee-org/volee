//! Behaviour of the rules from the outside: sandbox, merge, defaults,
//! fallbacks and a realistic file.

use std::time::Duration;

use volee_rules::{Change, FileChange, FinishedFacts, Layer, Limits, Risk, Rules, Source};

fn rules(repo: &str) -> Rules {
    Rules::from_sources(Some(Source::new("repo/.volee/rules.lua", repo)), None)
}

fn both(repo: &str, personal: &str) -> Rules {
    Rules::from_sources(
        Some(Source::new("repo/.volee/rules.lua", repo)),
        Some(Source::new("config/rules.lua", personal)),
    )
}

fn change(files: &[(&str, u32, u32)]) -> Change {
    Change {
        repo: "volee".into(),
        branch: "feat".into(),
        base: "main".into(),
        files: files
            .iter()
            .map(|(p, a, r)| FileChange {
                path: (*p).into(),
                added: *a,
                removed: *r,
            })
            .collect(),
        commits: 1,
        tests_touched: true,
        migrations: false,
        agent: "claude".into(),
        developer: "jeff".into(),
    }
}

fn assert_safest(rules: &Rules, expected_in_error: &str) {
    let c = change(&[("a.rs", 1, 0)]);
    let e = rules.evaluate(&c);
    assert_eq!(e.risk.level, Risk::High);
    assert!(!e.merge.allowed);
    assert_eq!(e.layers, Layer::ALL);
    assert!(e.review.reviewers >= 2);
    let all = e
        .errors
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(all.contains(expected_in_error), "errors were: {all}");
}

// ---- sandbox ---------------------------------------------------------

#[test]
fn forbidden_libraries_are_not_there() {
    for code in [
        "io.open('/etc/passwd')",
        "os.execute('true')",
        "os.getenv('HOME')",
        "os.remove('x')",
        "require('os')",
        "package.loadlib('x', 'y')",
        "debug.getinfo(1)",
        "dofile('x')",
        "loadfile('x')",
        "load('return 1')",
        "load(string.dump(function() end))",
        "string.dump(function() end)",
        "coroutine.wrap(function() end)()",
        "collectgarbage()",
    ] {
        let r = rules(&format!("local _ = {code}\nreturn {{}}"));
        assert!(r.is_degraded(), "should be refused: {code}");
        assert_eq!(r.errors()[0].line, Some(1), "{code}");
    }
}

#[test]
fn time_clock_and_date_are_allowed() {
    let r = rules("local _ = os.time() + os.clock() + #os.date('%Y')\nreturn {}");
    assert!(!r.is_degraded(), "{:?}", r.errors());
}

#[test]
fn bytecode_is_refused() {
    // The Lua signature byte of a binary chunk.
    let r = Rules::from_sources(Some(Source::new("bin.lua", "\x1bLua")), None);
    assert!(r.is_degraded());
}

#[test]
fn an_infinite_loop_is_stopped_at_load_and_at_run_time() {
    let r = Rules::from_sources_with(
        Some(Source::new("loop.lua", "while true do end")),
        None,
        Limits::default(),
    );
    assert_safest(&r, "too many instructions");

    let r = rules("return { risk = function(c) while true do end end }");
    assert!(!r.is_degraded());
    let out = r.risk(&change(&[("a.rs", 1, 0)]));
    assert_eq!(out.value.level, Risk::High);
    let error = out.error.expect("the loop must be reported").to_string();
    assert!(error.contains("repo/.volee/rules.lua:1"), "{error}");
    assert!(error.contains("risk"), "{error}");
}

#[test]
fn runaway_recursion_is_an_error_not_a_crash() {
    let r = rules(
        "return { risk = function(c)
           local function f(n) return 1 + f(n + 1) end
           return f(0)
         end }",
    );
    let out = r.risk(&change(&[("a.rs", 1, 0)]));
    assert_eq!(out.value.level, Risk::High);
    assert!(out.error.is_some());
}

#[test]
fn a_loop_cannot_hide_behind_pcall() {
    let r = rules(
        "return { risk = function(c)
           while true do pcall(function() while true do end end) end
         end }",
    );
    let out = r.risk(&change(&[("a.rs", 1, 0)]));
    assert_eq!(out.value.level, Risk::High);
    assert!(out.error.is_some());
}

#[test]
fn a_slow_rule_hits_the_time_budget() {
    let limits = Limits {
        instructions: u64::MAX,
        time: Duration::from_millis(50),
        ..Limits::default()
    };
    let r = Rules::from_sources_with(
        Some(Source::new(
            "slow.lua",
            "return { risk = function(c) while true do local x = os.clock() end end }",
        )),
        None,
        limits,
    );
    let out = r.risk(&change(&[("a.rs", 1, 0)]));
    assert_eq!(out.value.level, Risk::High);
    assert!(out.error.unwrap().message.contains("too long"));
}

#[test]
fn a_memory_bomb_is_stopped() {
    let r = rules(
        "return { risk = function(c)
           local t = {}
           for i = 1, 1e9 do t[i] = string.rep('x', 1000) .. i end
         end }",
    );
    let out = r.risk(&change(&[("a.rs", 1, 0)]));
    assert_eq!(out.value.level, Risk::High);
    assert!(out.error.unwrap().message.contains("memory"));

    let r = rules("return { risk = function(c) return string.rep('x', 1e10) end }");
    let out = r.risk(&change(&[("a.rs", 1, 0)]));
    assert!(out.error.is_some());
}

#[test]
fn the_rules_still_work_after_a_failed_call() {
    let r = rules(
        "return { risk = function(c)
           if c.branch == 'bomb' then while true do end end
           return 'low'
         end }",
    );
    let mut c = change(&[("a.rs", 1, 0)]);
    c.branch = "bomb".into();
    assert!(r.risk(&c).error.is_some());
    c.branch = "fine".into();
    let ok = r.risk(&c);
    assert_eq!(ok.value.level, Risk::Low);
    assert!(ok.error.is_none());
}

// ---- merge of the two files ------------------------------------------

#[test]
fn the_personal_file_overrides_and_extends_the_repo_file() {
    let r = both(
        r#"return {
             sensitive_paths = { "src/auth/**" },
             size = { medium_lines = 100, high_lines = 500 },
             review = { model = "haiku", reviewers = 1 },
             auto_merge = false,
             editors = {
               { name = "nvim", kind = "terminal", cmd = "nvim {file}" },
               { name = "code", kind = "external", cmd = "code {file}" },
             },
           }"#,
        r#"local base = ...
           return {
             sensitive_paths = { base.sensitive_paths[1], "src/payments/**" },
             size = { high_lines = 1000 },
             review = { model = "sonnet" },
             auto_merge = true,
             editors = { { name = "nvim", kind = "terminal", cmd = "nvim +{line} {file}" } },
           }"#,
    );
    assert!(!r.is_degraded(), "{:?}", r.errors());
    let s = r.settings();
    assert_eq!(s.sensitive_paths, ["src/auth/**", "src/payments/**"]);
    assert_eq!(s.size.medium_lines, 100, "untouched key kept");
    assert_eq!(s.size.high_lines, 1000, "overridden key");
    assert_eq!(s.review.model, "sonnet");
    assert_eq!(s.review.reviewers, 1, "table merged key by key");
    assert!(s.auto_merge);
    assert_eq!(s.editors.len(), 2, "editors merge by name");
    assert_eq!(s.editors[0].cmd, "nvim +{line} {file}");
    assert_eq!(s.editors[1].name, "code");
}

#[test]
fn lists_replace_when_the_personal_file_does_not_extend_them() {
    let r = both(
        r#"return { sensitive_paths = { "a/**", "b/**" } }"#,
        r#"return { sensitive_paths = { "c/**" } }"#,
    );
    assert_eq!(r.settings().sensitive_paths, ["c/**"]);
    let r = both(
        r#"return { sensitive_paths = { "a/**" } }"#,
        "return { sensitive_paths = {} }",
    );
    assert!(
        r.settings().sensitive_paths.is_empty(),
        "an empty list clears"
    );
}

#[test]
fn a_personal_function_replaces_the_repo_function_and_can_wrap_it() {
    let r = both(
        "return { risk = function(c) return 'medium', { 'repo says so' } end }",
        "local base = ...
         return { risk = function(c)
           local level, reasons = base.risk(c)
           if c.developer == 'jeff' then return 'low', { 'jeff knows this code' } end
           return level, reasons
         end }",
    );
    let c = change(&[("a.rs", 1, 0)]);
    let first = r.risk(&c);
    assert_eq!(first.value.level, Risk::Low, "{:?}", first.error);
    let mut other = c.clone();
    other.developer = "someone".into();
    let out = r.risk(&other).value;
    assert_eq!(
        (out.level, out.reasons),
        (Risk::Medium, vec!["repo says so".to_owned()])
    );
}

#[test]
fn either_file_may_be_missing() {
    let only_repo =
        Rules::from_sources(Some(Source::new("r", "return { auto_merge = true }")), None);
    assert!(only_repo.settings().auto_merge);
    let only_personal =
        Rules::from_sources(None, Some(Source::new("p", "return { auto_merge = true }")));
    assert!(only_personal.settings().auto_merge);
    let none = Rules::from_sources(None, None);
    assert!(!none.is_degraded());
    assert_eq!(none.settings(), &volee_rules::Settings::default());
}

#[test]
fn files_are_read_from_the_repo_and_the_config_dir() {
    let repo = tempfile::tempdir().unwrap();
    let config = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(repo.path().join(".volee")).unwrap();
    std::fs::write(
        repo.path().join(".volee/rules.lua"),
        "return { sensitive_paths = { 'x/**' } }",
    )
    .unwrap();
    std::fs::write(
        config.path().join("rules.lua"),
        "return { auto_merge = true }",
    )
    .unwrap();
    let r = Rules::load(Some(repo.path()), Some(config.path()));
    assert!(!r.is_degraded(), "{:?}", r.errors());
    assert_eq!(r.settings().sensitive_paths, ["x/**"]);
    assert!(r.settings().auto_merge);

    let nothing = Rules::load(Some(repo.path().join("nope").as_path()), None);
    assert!(!nothing.is_degraded(), "missing files are fine");
}

// ---- defaults --------------------------------------------------------

#[test]
fn with_no_file_the_defaults_come_from_rust() {
    let r = Rules::from_sources(None, None);
    let small = r.evaluate(&change(&[("src/a.rs", 5, 1)]));
    assert_eq!(small.risk.level, Risk::Low);
    assert_eq!(small.layers, [Layer::Summary, Layer::Diff]);
    assert_eq!(
        (small.review.model.as_str(), small.review.reviewers),
        ("haiku", 1)
    );
    assert!(!small.merge.allowed, "auto merge is off by default");
    assert_eq!(small.errors, []);

    let big = r.evaluate(&change(&[("src/a.rs", 900, 10)]));
    assert_eq!(big.risk.level, Risk::High);
    assert_eq!(big.layers, Layer::ALL);
    assert_eq!(big.review.reviewers, 2);
}

#[test]
fn settings_alone_drive_the_defaults() {
    let r = rules(
        r#"return {
             sensitive_paths = { "src/billing/**" },
             auto_merge = true,
             review = { model = "sonnet", reviewers = 0, high_reviewers = 3 },
             layers = { low = { "diff" } },
           }"#,
    );
    let low = r.evaluate(&change(&[("docs/a.md", 3, 0)]));
    assert_eq!(low.layers, [Layer::Diff]);
    assert_eq!(low.review.reviewers, 0);
    assert!(low.merge.allowed);
    let high = r.evaluate(&change(&[("src/billing/x.rs", 3, 0)]));
    assert_eq!(high.risk.level, Risk::High);
    assert_eq!(high.review.reviewers, 3);
    assert!(!high.merge.allowed);
}

#[test]
fn a_function_may_start_from_the_default_and_adjust() {
    let r = rules(
        r#"return {
             risk = function(c)
               local level, reasons = volee.default.risk(c)
               if c.agent == "claude" and level == "low" then
                 return "low", { "agent is trusted" }
               end
               return level, reasons
             end,
             layers = function(c, risk, reasons)
               if risk == "low" then return { "diff" } end
               return volee.default.layers(risk)
             end,
             review = function(c, risk) return { reviewers = 5 } end,
             auto_merge = function(c, risk) return risk == "low", "small and trusted" end,
           }"#,
    );
    let e = r.evaluate(&change(&[("a.rs", 1, 0)]));
    assert_eq!(e.risk.reasons, ["agent is trusted"]);
    assert_eq!(e.layers, [Layer::Diff]);
    assert_eq!((e.review.model.as_str(), e.review.reviewers), ("haiku", 5));
    assert!(e.merge.allowed);
    assert_eq!(e.merge.reason, "small and trusted");
}

#[test]
fn high_risk_is_never_merged_automatically_even_if_a_rule_says_so() {
    let r = rules(
        r#"return {
             risk = function(c) return "high", { "tested by hand" } end,
             auto_merge = function(c, risk) return true, "trust me" end,
           }"#,
    );
    let e = r.evaluate(&change(&[("a.rs", 1, 0)]));
    assert_eq!(e.risk.level, Risk::High);
    assert!(!e.merge.allowed);
}

#[test]
fn held_question_timeout_takes_seconds_or_text() {
    let secs = |s: &str| rules(&format!("return {{ held_question_timeout = {s} }}"));
    assert_eq!(
        secs("90").settings().held_question_timeout(),
        Duration::from_secs(90)
    );
    assert_eq!(
        secs("\"10m\"").settings().held_question_timeout(),
        Duration::from_secs(600)
    );
    assert_eq!(
        secs("\"2h\"").settings().held_question_timeout(),
        Duration::from_secs(7200)
    );
    assert_eq!(
        Rules::from_sources(None, None)
            .settings()
            .held_question_timeout(),
        Duration::from_secs(600)
    );
    assert!(secs("\"soon\"").is_degraded());
    assert!(secs("-5").is_degraded());
}

#[test]
fn finished_defaults_to_the_core_rule_and_a_function_replaces_it() {
    let facts = FinishedFacts {
        stopped_last: true,
        busy: false,
        commits: 2,
        unique_commits: 2,
        merged: false,
    };
    let none = Rules::from_sources(None, None);
    assert!(none.finished(&facts).value);
    let busy = FinishedFacts {
        busy: true,
        ..facts.clone()
    };
    assert!(!none.finished(&busy).value);
    let merged = FinishedFacts {
        merged: true,
        ..facts.clone()
    };
    assert!(!none.finished(&merged).value);

    let strict = rules("return { finished = function(f) return f.unique_commits >= 3 end }");
    assert!(!strict.finished(&facts).value);
    let three = FinishedFacts {
        unique_commits: 3,
        ..facts
    };
    assert!(strict.finished(&three).value);
}

#[test]
fn editors_become_commands() {
    let r = rules(
        r#"return { editors = {
             { name = "nvim", kind = "terminal", cmd = "nvim +{line} {file}" },
             { name = "code", kind = "external", cmd = "code -g {file}:{line}" },
           } }"#,
    );
    let target = volee_rules::EditorTarget {
        file: "/w/src/a b.rs".into(),
        line: 7,
        dir: "/w".into(),
        repo: "volee".into(),
    };
    let argv = r.editors()[1].command(&target).unwrap();
    assert_eq!(argv, ["code", "-g", "/w/src/a b.rs:7"]);
    assert!(
        rules(r#"return { editors = { { name = "x", kind = "terminal", cmd = "x {nope}" } } }"#)
            .is_degraded()
    );
}

// ---- fallbacks -------------------------------------------------------

#[test]
fn a_syntax_error_degrades_to_the_safest_outcome_with_file_and_line() {
    let r = rules("return {\n  auto_merge = true,\n  risk = function(c) return 'low' end end\n}");
    assert!(r.is_degraded());
    let error = &r.errors()[0];
    assert_eq!(error.file.as_deref(), Some("repo/.volee/rules.lua"));
    assert!(error.line.is_some());
    assert_safest(&r, "repo/.volee/rules.lua:");
    // The broken file's auto_merge = true is not honoured.
    assert!(!r.settings().auto_merge);
}

#[test]
fn a_load_error_in_the_personal_file_is_reported_with_its_own_name() {
    let r = both("return {}", "local x = nil\nreturn { a = x.b }");
    assert!(r.is_degraded());
    assert_eq!(r.errors()[0].file.as_deref(), Some("config/rules.lua"));
    assert_eq!(r.errors()[0].line, Some(2));
    assert_safest(&r, "config/rules.lua:2");
}

#[test]
fn a_file_that_does_not_return_a_table_is_an_error() {
    let r = rules("return 42");
    assert_safest(&r, "must return a table");
    let r = rules("local x = 1");
    assert!(r.is_degraded());
}

#[test]
fn bad_settings_are_an_error_not_a_silent_default() {
    for bad in [
        r#"return { size = { medium_lines = "many" } }"#,
        r#"return { layers = { low = { "nonsense" } } }"#,
        r#"return { auto_merge = "yes" }"#,
        r#"return { risk = "high" }"#,
    ] {
        let r = rules(bad);
        assert!(r.is_degraded(), "{bad}");
        assert_safest(&r, "");
    }
}

#[test]
fn an_unknown_key_is_a_warning_not_an_error() {
    let r = rules("return { sensitive_pathz = { 'x' } }");
    assert!(!r.is_degraded());
    assert!(r.warnings()[0].contains("sensitive_pathz"));
}

#[test]
fn each_function_that_fails_answers_the_safest_for_its_own_question() {
    let c = change(&[("a.rs", 1, 0)]);
    let low = volee_rules::RiskAssessment {
        level: Risk::Low,
        reasons: vec![],
    };

    let r = rules("return { risk = function(c) error('boom') end }");
    let out = r.risk(&c);
    assert_eq!(out.value.level, Risk::High);
    assert!(out.error.unwrap().to_string().contains("boom"));

    let r = rules("return { layers = function(c, r) return { 'diff', 'oops' } end }");
    let out = r.layers(&c, &low);
    assert_eq!(out.value, Layer::ALL);
    assert!(out.error.unwrap().message.contains("oops"));

    let r = rules("return { review = function(c, r) return nil end }");
    let out = r.review(&c, &low);
    assert_eq!(out.value.reviewers, 2);
    assert!(out.error.is_some());

    let r = rules("return { auto_merge = function(c, r) return 'yes' end }");
    let out = r.auto_merge(&c, &low);
    assert!(!out.value.allowed);
    assert!(out.error.is_some());

    let r = rules("return { risk = function(c) return 'critical' end }");
    let out = r.risk(&c);
    assert_eq!(out.value.level, Risk::High);
    let error = out.error.unwrap();
    assert_eq!(error.line, Some(1));
    assert_eq!(error.function.as_deref(), Some("risk"));
}

#[test]
fn a_failing_finished_function_falls_back_to_the_builtin_rule() {
    let facts = FinishedFacts {
        stopped_last: true,
        busy: false,
        commits: 1,
        unique_commits: 1,
        merged: false,
    };
    let r = rules("return { finished = function(f) error('nope') end }");
    let out = r.finished(&facts);
    assert!(out.value);
    assert!(out.error.is_some());
}

#[test]
fn a_failing_risk_function_makes_the_whole_evaluation_safe() {
    let r = rules(
        "return { auto_merge = true,
                  risk = function(c) return c.nothing.here end }",
    );
    let e = r.evaluate(&change(&[("a.rs", 1, 0)]));
    assert_eq!(e.risk.level, Risk::High);
    assert!(!e.merge.allowed, "high risk is never automatic");
    assert_eq!(e.review.reviewers, 2);
    assert_eq!(e.errors.len(), 1);
    assert_eq!(e.errors[0].line, Some(2));
}

#[test]
fn rules_can_move_between_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<Rules>();
}

// ---- a realistic file ------------------------------------------------

const REALISTIC: &str = r#"
-- .volee/rules.lua of a web shop
local sensitive = { "src/payments/**", "src/auth/**", "infra/**", ".github/workflows/**" }

return {
  sensitive_paths = sensitive,
  size = { medium_lines = 150, high_lines = 600 },
  review = { model = "haiku", reviewers = 1, high_reviewers = 2 },
  held_question_timeout = "20m",
  editors = {
    { name = "nvim", kind = "terminal", cmd = "nvim +{line} {file}" },
    { name = "code", kind = "external", cmd = "code -g {file}:{line}" },
  },

  risk = function(change)
    local level, reasons = volee.default.risk(change)
    -- docs only: always low
    local only_docs = true
    for _, f in ipairs(change.files) do
      if not (volee.glob("docs/**", f.path) or volee.glob("*.md", f.path)) then
        only_docs = false
      end
    end
    if only_docs then return "low", { "documentation only" } end
    -- a lockfile bump alone is boring; with code it is not
    if change.migrations and not change.tests_touched then
      return "high", { "migration without a test" }
    end
    return level, reasons
  end,

  auto_merge = function(change, risk)
    if risk ~= "low" then return false, "needs a person at " .. risk end
    if change.agent ~= "claude" then return false, "unknown agent" end
    return true, "low risk change by claude"
  end,
}
"#;

#[test]
fn a_realistic_rules_file_decides_as_written() {
    let r = rules(REALISTIC);
    assert!(r.errors().is_empty(), "{:?}", r.errors());
    assert_eq!(r.warnings(), &[] as &[String]);

    let docs = r.evaluate(&change(&[("docs/guide.md", 40, 2), ("README.md", 1, 1)]));
    assert_eq!(docs.risk.level, Risk::Low);
    assert!(docs.merge.allowed);
    assert_eq!(docs.merge.reason, "low risk change by claude");

    let payments = r.evaluate(&change(&[("src/payments/charge.rs", 12, 3)]));
    assert_eq!(payments.risk.level, Risk::High);
    assert!(payments.risk.reasons[0].contains("src/payments/charge.rs"));
    assert!(!payments.merge.allowed);
    assert_eq!(payments.layers, Layer::ALL);
    assert_eq!(payments.review.reviewers, 2);

    let mut migration = change(&[("db/migrations/0042.sql", 10, 0)]);
    migration.migrations = true;
    migration.tests_touched = false;
    let m = r.evaluate(&migration);
    assert_eq!(m.risk.reasons, ["migration without a test"]);

    let mut other_agent = change(&[("docs/a.md", 1, 0)]);
    other_agent.agent = "codex".into();
    assert_eq!(r.evaluate(&other_agent).merge.reason, "unknown agent");

    assert_eq!(
        r.settings().held_question_timeout(),
        Duration::from_secs(1200)
    );
    assert_eq!(r.editors().len(), 2);
}
