import fs from "node:fs/promises";
import os from "node:os";
import path from "node:path";

import { afterEach, beforeEach } from "@jest/globals";

const originalCodexHome = process.env.DSX_HOME;
let currentCodexHome: string | undefined;

beforeEach(async () => {
  currentCodexHome = await fs.mkdtemp(path.join(os.tmpdir(), "codex-sdk-test-"));
  process.env.DSX_HOME = currentCodexHome;
});

afterEach(async () => {
  const codexHomeToDelete = currentCodexHome;
  currentCodexHome = undefined;

  if (originalCodexHome === undefined) {
    delete process.env.DSX_HOME;
  } else {
    process.env.DSX_HOME = originalCodexHome;
  }

  if (codexHomeToDelete) {
    await fs.rm(codexHomeToDelete, { recursive: true, force: true });
  }
});
