import { readdirSync } from "node:fs";
import { join } from "node:path";

/** Recursive file walk. Hand-rolled to mirror Rust's walkdir for a fair comparison. */
export function* walk(dir: string): Generator<string> {
  let entries;
  try {
    entries = readdirSync(dir, { withFileTypes: true });
  } catch {
    return; // missing or unreadable directory is not fatal
  }
  for (const e of entries) {
    const p = join(dir, e.name);
    if (e.isDirectory()) yield* walk(p);
    else if (e.isFile()) yield p;
  }
}
