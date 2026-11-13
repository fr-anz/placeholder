export const VERSION = "0.1.0";
export const REPO = "https://github.com/fr-anz/placeholder";
export const RELEASES = `${REPO}/releases`;

/** The agent CLIs the app can drive, in the order the page lists them. */
export const AGENTS = [
  { name: "Claude Code", logo: "claude" },
  { name: "Codex", logo: "codex-glyph" },
  { name: "Cursor", logo: "cursor" },
  { name: "OpenCode", logo: "opencode" },
  { name: "Grok Build", logo: "grok" },
  { name: "Devin", logo: "devin" },
  { name: "Antigravity", logo: "antigravity" },
  { name: "Pi", logo: "pi" },
  { name: "Hermes Agent", logo: "hermes" },
  { name: "omp", logo: "omp" },
  { name: "fx", logo: "fx" },
] as const;

export const SPEC: { term: string; detail: string }[] = [
  { term: "Terminal", detail: "Built-in terminal, real PTY." },
  { term: "Editor", detail: "Syntax highlighting, folds and diagnostics." },
  { term: "Git", detail: "Branches, worktrees, staging and commits." },
  { term: "Checkpoints", detail: "Keep or undo each turn’s changes." },
  { term: "Inbox", detail: "Issues from your existing trackers." },
  { term: "Accounts", detail: "Switch accounts when you hit a limit." },
  { term: "Usage", detail: "Track usage and stop runaway tools." },
  { term: "Remotes", detail: "Run agents on other machines over SSH." },
  { term: "Notes", detail: "Save replies or send them back." },
  { term: "/btw", detail: "Ask without interrupting the current task." },
  { term: "Search", detail: "Find anything across your project." },
  { term: "Native", detail: "A native desktop app, built with GPUI." },
];

// Asset names match .github/workflows/release.yml and scripts/package-*.sh.
export const BUILDS = [
  {
    os: "Linux",
    kind: "AppImage",
    arch: "Intel & AMD · 64-bit",
    href: `${RELEASES}/download/v${VERSION}/Placeholder-${VERSION}-x86_64.AppImage`,
    install: "Make executable, then open.",
  },
  {
    os: "Windows",
    kind: ".exe",
    arch: "Intel & AMD · 64-bit",
    href: `${RELEASES}/download/v${VERSION}/Placeholder-${VERSION}-x86_64.exe`,
    install: "Open the downloaded executable.",
  },
  {
    os: "macOS",
    kind: ".dmg",
    arch: "Apple silicon & Intel",
    href: `${RELEASES}/download/v${VERSION}/Placeholder-${VERSION}-macos-universal.dmg`,
    install: "Drag Placeholder into Applications.",
  },
] as const;
