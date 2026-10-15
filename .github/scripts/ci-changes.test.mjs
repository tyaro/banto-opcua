import assert from "node:assert/strict";
import test from "node:test";

import { classify } from "./ci-changes.mjs";

/**
 * @param {string[]} files
 * @param {{ rust: boolean, deny: boolean }} want
 */
function pr(files, want) {
  const { jobs } = classify({ event: "pull_request", files });
  assert.equal(jobs.run_rust, want.rust, `rust ${files.join(",")}`);
  assert.equal(jobs.run_deny, want.deny, `deny ${files.join(",")}`);
  if (want.rust) assert.equal(jobs.rust_args, "--workspace");
}

test("docs だけの PR は Rust も deny も走らせない", () => {
  pr(["README.md", "docs/adr/0001-async-opcua-base.md", "LICENSE"], {
    rust: false,
    deny: false,
  });
});

test("クレートと Cargo の変更は workspace 全体", () => {
  pr(["crates/banto-opcua-core/src/lib.rs"], { rust: true, deny: true });
  pr(["Cargo.toml", "Cargo.lock"], { rust: true, deny: true });
  pr(["deny.toml"], { rust: true, deny: true });
  pr(["rust-toolchain.toml"], { rust: true, deny: true });
});

test("ワークフロー自身と未知のパスはフル CI", () => {
  pr([".github/workflows/ci.yml"], { rust: true, deny: true });
  pr(["scripts/unexpected.sh"], { rust: true, deny: true });
});

test("文書とクレートが混ざったら Rust を走らせる", () => {
  pr(["README.md", "crates/banto-opcua-server/src/lib.rs"], { rust: true, deny: true });
});

test("push は差分が空でもフル CI", () => {
  const { jobs } = classify({ event: "push", files: [] });
  assert.equal(jobs.run_rust, true);
  assert.equal(jobs.run_deny, true);
});

test("差分が取れない PR はフル CI", () => {
  const { jobs, forced } = classify({ event: "pull_request", files: null });
  assert.ok(forced);
  assert.equal(jobs.run_rust, true);
});
