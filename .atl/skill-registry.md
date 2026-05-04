# Skill Registry

**Delegator use only.** Any agent that launches sub-agents reads this registry to resolve compact rules, then injects them directly into sub-agent prompts. Sub-agents do NOT read this registry or individual SKILL.md files.

See `_shared/skill-resolver.md` for the full resolution protocol.

## User Skills

| Trigger | Skill | Path |
|---------|-------|------|
| Browser automation, navigate pages, fill forms, click buttons, take screenshots, scrape data, automate browser tasks | agent-browser | ~/.agents/skills/agent-browser/SKILL.md |
| Braintrust tracing for Claude Code, hook architecture, sub-agent correlation | braintrust-tracing | ~/.agents/skills/braintrust-tracing/SKILL.md |
| PR creation, opening a pull request, preparing changes for review | branch-pr | ~/.config/opencode/skills/branch-pr/SKILL.md |
| Browser automation, debugging, performance analysis with Puppeteer CLI, screenshots, network traffic, web scraping | chrome-devtools | ~/.agents/skills/chrome-devtools/SKILL.md |
| Debugging, profiling, root cause analysis, bugs, performance issues, unexpected behavior | debugging-strategies | ~/.agents/skills/debugging-strategies/SKILL.md |
| Writing, reviewing, editing docs in /docs directory or .md files | docs-writer | ~/.agents/skills/docs-writer/SKILL.md |
| How do I do X, find a skill for X, is there a skill that can..., extending capabilities | find-skills | ~/.agents/skills/find-skills/SKILL.md |
| Go testing, Bubbletea TUI testing, teatest, test coverage | go-testing | ~/.config/opencode/skills/go-testing/SKILL.md |
| Creating a GitHub issue, reporting a bug, requesting a feature | issue-creation | ~/.config/opencode/skills/issue-creation/SKILL.md |
| judgment day, judgment-day, review adversarial, dual review, doble review, juzgar, que lo juzguen | judgment-day | ~/.config/opencode/skills/judgment-day/SKILL.md |
| Navigate websites, fill forms, take screenshots, test web apps, extract data from web pages | playwright-cli | ~/.agents/skills/playwright-cli/SKILL.md |
| /call-graph, call hierarchy, who calls, what calls | rust-call-graph | ~/.agents/skills/rust-call-graph/SKILL.md |
| /refactor, rename symbol, move function, extract, safe refactoring | rust-refactor-helper | ~/.agents/skills/rust-refactor-helper/SKILL.md |
| /symbols, project structure, list structs, list traits, list functions | rust-symbol-analyzer | ~/.agents/skills/rust-symbol-analyzer/SKILL.md |
| Rust testing, cargo test, mockall, proptest, tokio test, test organization | rust-testing | ~/.agents/skills/rust-testing/SKILL.md |
| Create a new skill, add agent instructions, document patterns for AI | skill-creator | ~/.config/opencode/skills/skill-creator/SKILL.md |
| UI/UX design, web and mobile, plan/build/create/design/implement/review/fix/improve UI/UX code | ui-ux-pro-max | ~/.agents/skills/ui-ux-pro-max/SKILL.md |

## Compact Rules

Pre-digested rules per skill. Delegators copy matching blocks into sub-agent prompts as `## Project Standards (auto-resolved)`.

### rust-testing
- Use `#[cfg(test)] mod tests` for unit tests, `tests/` directory for integration tests
- Mock dependencies via traits (trait-based design) — prefer `mockall::automock` for complex mocking
- Use `#[tokio::test]` for async test functions; use `tokio::test(start_paused = true)` for timeout testing
- Use `proptest` for property-based testing with custom strategies for domain types
- Use `rstest` for fixtures and parametrized tests; `TempDir` for file system isolation
- Assert specific error types (`assert!(matches!(result, Err(Error::NotFound)))`), not just `is_err()`
- Avoid real I/O in unit tests — mock external services; benchmark critical paths with Criterion
- Extract logic from `main.rs` into `lib.rs` for testability in binary crates

### rust-refactor-helper
- Always run `--dry-run` first to preview impact before applying changes
- For rename: find ALL references (LSP findReferences), categorize by file, check for conflicts
- For extract function: identify inputs/outputs/local variables, check for early returns and loops
- For move symbol: find dependencies and callers, check for circular dependencies, generate import changes
- Safety checks: reference completeness, name conflicts, visibility changes, macro-generated code, documentation references

### rust-call-graph
- Use LSP `prepareCallHierarchy` to start, then `incomingCalls` (callers) or `outgoingCalls` (callees)
- Max depth 3 by default; use `--depth N` for deeper analysis
- Direction: `in` for who-calls-this, `out` for what-this-calls, `both` for complete graph
- Output as ASCII tree or Mermaid diagram; highlight hot paths (high fan-in functions)

### rust-symbol-analyzer
- Use LSP `documentSymbol` for single file, `workspaceSymbol` for entire project
- Filter by type: `--type struct|trait|fn|mod|enum`
- Show hierarchical structure (modules → types → methods); detect orphan symbols
- Works with workspace-level projects — scans all crates

### debugging-strategies
- Scientific method: Observe → Hypothesize → Experiment → Analyze → Repeat
- Reproduce consistently first; create minimal reproduction; isolate the problem
- Gather: full stack trace, environment details, recent changes (git history), scope (affected users)
- Binary search: comment out half the code to narrow down; add strategic logging; isolate components
- Don't assume "it can't be X" — check anyway; question everything

### docs-writer
- For files in `/docs/` or `.md` files in repo: use active voice, present tense, address dev as "you"
- BLUF (Bottom Line Up Front): start with domain purpose before technical details
- Use sentence case for headings, serial comma, wrap at 80 chars (except code blocks)
- Domain terms capitalized consistently; Rust types in `backticks`
- DDD structure: Domain Concept → Rust Implementation → Examples; document invariants and domain events

### branch-pr
- Follow issue-first enforcement: PR must reference an existing issue
- Verify branch is synced with base before PR creation
- Include summary, test plan, and affected modules in PR description

### issue-creation
- Use structured format: description, steps to reproduce, expected vs actual behavior, environment
- Label appropriately (bug, enhancement, docs, etc.)
- Link to related issues/epics

### judgment-day
- Two independent blind judges review simultaneously, synthesize findings, apply fixes
- Re-judge until both pass, max 2 iterations; escalate if deadlock

### skill-creator
- Follow Agent Skills spec format: frontmatter with name, description, triggers, allowed-tools
- Keep SKILL.md concise — actionable rules, not tutorials
- Test new skill by loading it and verifying triggers activate correctly

### agent-browser
- Use CLI commands for browser automation: navigate, click, type, screenshot, extract
- Wait for selectors before interacting; handle dynamic content loading
- Take screenshots for visual verification of results

### chrome-devtools
- Use Puppeteer CLI scripts for automation; supports performance tracing and network monitoring
- Screenshots, form automation, JS debugging via DevTools protocol
- Handle authentication flows and cookie management

### playwright-cli
- Automates browsers via Playwright: navigation, forms, screenshots, data extraction
- Supports multiple browsers (Chromium, Firefox, WebKit)
- Use locators for reliable element targeting

### go-testing
- Use `teatest` for Bubbletea TUI testing; standard `go test` for unit/integration
- Table-driven tests preferred; subtests with `t.Run()` for organization
- Mock interfaces with generated mocks or manual test doubles

### braintrust-tracing
- Hook architecture for tracing; sub-agent correlation via trace IDs
- Debug tracing issues by examining hook execution order

### find-skills
- Search available skills by capability or keyword; suggest installation when not found
- Report skill name, description, and install location

### ui-ux-pro-max
- 50+ styles, 161 palettes, 57 font pairings, 99 UX guidelines, 25 chart types
- Stacks: React, Next.js, Vue, Svelte, SwiftUI, React Native, Flutter, Tailwind, shadcn/ui, HTML/CSS
- Actions: plan, build, create, design, implement, review, fix, improve, optimize, enhance, refactor, check
- Styles: glassmorphism, claymorphism, minimalism, brutalism, neumorphism, bento grid, dark mode, responsive

## Project Conventions

| File | Path | Notes |
|------|------|-------|
| CONTRIBUTING.md | /home/rubentxu/Proyectos/rust/Bastion/CONTRIBUTING.md | Commit conventions (feat/fix/docs/refactor/test), PR checklist, code style |
| .cargo/config.toml | /home/rubentxu/Proyectos/rust/Bastion/.cargo/config.toml | Build profiles: MUSL static linking, release opt-level=s, LTO, strip, panic=abort |
| docs/architecture.md | /home/rubentxu/Proyectos/rust/Bastion/docs/architecture.md | DDD + Clean Architecture reference, worker protocol v2, security & reliability models |
