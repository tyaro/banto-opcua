// CI の変更パス判定。`.github/workflows/ci.yml` の changes ジョブが呼ぶ。
//
// banto-industrial の ci-changes.mjs（#468）を、Rust だけのリポジトリ向けに
// 縮めたもの。判定は純関数で、ci-changes.test.mjs が表を検証する。
// どの規則にも当たらないパスはフル CI（必要なテストが走らない側に倒さない）。

import { appendFileSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { fileURLToPath } from "node:url";

/** @type {string[]} */
export const AREAS = ["full", "docs", "rust"];

/**
 * パス → 領域。上から順に、最初に当たったものを採る。
 * @type {Array<{ re: RegExp, areas: string[], why: string }>}
 */
export const RULES = [
  { re: /^\.github\//, areas: ["full"], why: "CI 定義・判定スクリプト自身" },
  { re: /^\.gitattributes$/, areas: ["full"], why: "改行（fmt に効く）" },
  {
    re: /^(crates|interop)\//,
    areas: ["rust"],
    why: "クレート、または相互接続の準備",
  },
  {
    re: /^(Cargo\.toml|Cargo\.lock|rust-toolchain(\.toml)?|deny\.toml|\.?rustfmt\.toml|\.?clippy\.toml)$/,
    areas: ["rust"],
    why: "workspace / toolchain / lint / deny",
  },
  { re: /^docs\//, areas: ["docs"], why: "docs" },
  { re: /^[^/]+\.md$/, areas: ["docs"], why: "root の Markdown" },
  { re: /^(LICENSE|\.gitignore)$/, areas: ["docs"], why: "ライセンス・ignore" },
  { re: /^\.cursor\//, areas: ["docs"], why: "エージェント向け設定（CI は読まない）" },
];

/**
 * @param {string[]} files
 */
export function classifyAreas(files) {
  /** @type {Record<string, boolean>} */
  const areas = Object.fromEntries(AREAS.map((area) => [area, false]));
  const matches = [];
  for (const raw of files) {
    const file = raw.trim().replace(/\\/g, "/");
    if (file === "") continue;
    const rule = RULES.find((candidate) => candidate.re.test(file));
    const hit = rule
      ? { file, areas: rule.areas, why: rule.why }
      : { file, areas: ["full"], why: "どの規則にも当たらない（安全側でフル CI）" };
    for (const area of hit.areas) areas[area] = true;
    matches.push(hit);
  }
  return { areas, matches };
}

/**
 * @param {Record<string, boolean>} areas
 */
export function derive(areas) {
  const run = areas.full || areas.rust;
  return {
    run_rust: run,
    run_deny: run,
    rust_args: run ? "--workspace" : "",
  };
}

/**
 * @param {{ event: string, files: string[] | null }} input
 */
export function classify({ event, files }) {
  const forced =
    event !== "pull_request"
      ? `${event} イベント（PR 以外）はフル CI`
      : files === null
        ? "差分を取得できなかった（安全側でフル CI）"
        : null;
  const { areas, matches } = classifyAreas(files ?? []);
  if (forced) areas.full = true;
  return { areas, matches, forced, jobs: derive(areas) };
}

function main() {
  const event = process.env.CI_EVENT ?? "";
  const filesPath = process.env.CI_CHANGED_FILES;
  /** @type {string[] | null} */
  let files = null;
  if (filesPath && process.env.CI_DIFF_OK === "true") {
    files = readFileSync(filesPath, "utf8").split("\n");
  }
  const result = classify({ event, files });
  const lines = [
    ...AREAS.map((area) => `${area}=${result.areas[area]}`),
    ...Object.entries(result.jobs).map(([key, value]) => `${key}=${value}`),
  ];
  console.log(lines.join("\n"));
  if (process.env.GITHUB_OUTPUT) {
    appendFileSync(process.env.GITHUB_OUTPUT, `${lines.join("\n")}\n`);
  }
  const summary = [
    "## 変更パス判定",
    "",
    result.forced ? `**${result.forced}**` : `変更ファイル ${result.matches.length} 件`,
    "",
    "| 出力 | 値 |",
    "| --- | --- |",
    ...lines.map((line) => {
      const [key, ...rest] = line.split("=");
      return `| \`${key}\` | \`${rest.join("=") || "(空)"}\` |`;
    }),
    "",
  ];
  if (process.env.GITHUB_STEP_SUMMARY) {
    appendFileSync(process.env.GITHUB_STEP_SUMMARY, `${summary.join("\n")}\n`);
  }
}

if (process.argv[1] && resolve(fileURLToPath(import.meta.url)) === resolve(process.argv[1])) {
  main();
}
