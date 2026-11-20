#!/usr/bin/env node
// Собирает CLI `rmon` и кладёт его туда, где Tauri ждёт sidecar:
// apps/rmon-app/src-tauri/binaries/rmon-<target-triple>[.exe] (bundle.externalBin).
// Запускается из beforeDevCommand/beforeBuildCommand; цель берётся из
// TAURI_ENV_TARGET_TRIPLE (её задаёт tauri для `--target`), иначе — хост rustc.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, chmodSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const release = process.argv.includes("--release") || process.env.TAURI_ENV_DEBUG === "false";
const host = execFileSync("rustc", ["-vV"], { encoding: "utf8" }).match(/^host: (.+)$/m)[1];
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || host;
const exe = triple.includes("windows") ? ".exe" : "";

const args = ["build", "-p", "rmon-cli", "--locked"];
if (release) args.push("--release");
if (triple !== host) args.push("--target", triple);
execFileSync("cargo", args, { cwd: root, stdio: "inherit" });

const profile = release ? "release" : "debug";
const built = join(root, "target", triple === host ? "" : triple, profile, `rmon${exe}`);
const dest = join(root, "apps/rmon-app/src-tauri/binaries", `rmon-${triple}${exe}`);
mkdirSync(dirname(dest), { recursive: true });
copyFileSync(built, dest);
chmodSync(dest, 0o755);
console.log(`sidecar: ${dest}`);
