import * as child_process from "node:child_process";
import { EventEmitter } from "node:events";
import path from "node:path";
import { PassThrough } from "node:stream";

import { describe, expect, it } from "@jest/globals";

jest.mock("node:child_process", () => {
  const actual = jest.requireActual<typeof import("node:child_process")>("node:child_process");
  return { ...actual, spawn: jest.fn() };
});

const _actualChildProcess =
  jest.requireActual<typeof import("node:child_process")>("node:child_process");
const spawnMock = child_process.spawn as jest.MockedFunction<typeof _actualChildProcess.spawn>;

function pathDelimiterFor(platform: NodeJS.Platform): string {
  return platform === "win32" ? ";" : ":";
}

async function withMockedPlatform<T>(platform: NodeJS.Platform, fn: () => Promise<T>): Promise<T> {
  const originalDescriptor = Object.getOwnPropertyDescriptor(process, "platform");
  Object.defineProperty(process, "platform", {
    configurable: true,
    value: platform,
  });

  try {
    return await fn();
  } finally {
    if (originalDescriptor) {
      Object.defineProperty(process, "platform", originalDescriptor);
    }
  }
}

class FakeChildProcess extends EventEmitter {
  stdin = new PassThrough();
  stdout = new PassThrough();
  stderr = new PassThrough();
  killed = false;

  kill(): boolean {
    this.killed = true;
    return true;
  }
}

function createEarlyExitChild(exitCode = 2): FakeChildProcess {
  const child = new FakeChildProcess();
  setImmediate(() => {
    child.stderr.write("boom");
    child.emit("exit", exitCode, null);
    setImmediate(() => {
      child.stdout.end();
      child.stderr.end();
    });
  });
  return child;
}

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

describe("CodexExec", () => {
  describe("_findCodexPathForTesting", () => {
    it("prefers mcodex from PATH when no explicit executable is provided", async () => {
      const { _findCodexPathForTesting } = await import("../src/exec");
      const platform = "darwin";
      const envPath = ["/usr/local/bin", "/opt/homebrew/bin"].join(pathDelimiterFor(platform));
      const expected = path.posix.join("/opt/homebrew/bin", "mcodex");

      const result = _findCodexPathForTesting({
        envPath,
        platform,
        arch: "arm64",
        pathExists: (candidate: string) => candidate === expected,
        resolvePackageJson: () => {
          throw new Error("npm fallback should not be used when PATH contains mcodex");
        },
      });

      expect(result).toBe(expected);
    });

    it("reports script-install guidance when no CLI can be found", async () => {
      const { _findCodexPathForTesting } = await import("../src/exec");

      expect(() =>
        _findCodexPathForTesting({
          envPath: "/usr/local/bin:/opt/homebrew/bin",
          platform: "linux",
          arch: "x64",
          pathExists: () => false,
          resolvePackageJson: () => {
            throw new Error("missing package");
          },
        }),
      ).toThrow(/install\.sh.*install\.ps1.*explicit executable path/i);
    });

    it("prefers mcodex.exe on Windows", async () => {
      const { _findCodexPathForTesting } = await import("../src/exec");
      const platform = "win32";
      const envPath = ["C:\\wrapper", "D:\\bin"].join(pathDelimiterFor(platform));
      const expected = path.win32.join("D:\\bin", "mcodex.exe");
      const wrapperPath = path.win32.join("C:\\wrapper", "mcodex.ps1");

      const result = _findCodexPathForTesting({
        envPath,
        platform,
        arch: "x64",
        pathExists: (candidate: string) => candidate === expected || candidate === wrapperPath,
        resolvePackageJson: () => {
          throw new Error("npm fallback should not be used when PATH contains mcodex.exe");
        },
      });

      expect(result).toBe(expected);
    });

    it("finds mcodex.ps1 on Windows when mcodex.exe is absent", async () => {
      const { _findCodexPathForTesting } = await import("../src/exec");
      const platform = "win32";
      const envPath = ["C:\\Users\\me\\AppData\\Local\\Programs\\Mcodex\\bin"].join(
        pathDelimiterFor(platform),
      );
      const expected = path.win32.join(
        "C:\\Users\\me\\AppData\\Local\\Programs\\Mcodex\\bin",
        "mcodex.ps1",
      );

      const result = _findCodexPathForTesting({
        envPath,
        platform,
        arch: "x64",
        pathExists: (candidate: string) => candidate === expected,
        resolvePackageJson: () => {
          throw new Error("npm fallback should not be used when PATH contains mcodex.ps1");
        },
      });

      expect(result).toBe(expected);
    });

    it("falls back to the legacy npm package when PATH lookup fails", async () => {
      const { _findCodexPathForTesting } = await import("../src/exec");
      const resolvePackageJson = jest.fn((specifier: string, from?: string) => {
        if (specifier === "@openai/codex/package.json" && from === undefined) {
          return "/repo/node_modules/@openai/codex/package.json";
        }
        if (
          specifier === "@openai/codex-linux-x64/package.json" &&
          from === "/repo/node_modules/@openai/codex/package.json"
        ) {
          return "/repo/node_modules/@openai/codex-linux-x64/package.json";
        }
        throw new Error(`Unexpected package lookup: ${specifier} from ${from}`);
      });

      const result = _findCodexPathForTesting({
        envPath: "/usr/local/bin:/opt/bin",
        platform: "linux",
        arch: "x64",
        pathExists: () => false,
        resolvePackageJson,
      });

      expect(result).toBe(
        "/repo/node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/codex/codex",
      );
      expect(resolvePackageJson).toHaveBeenNthCalledWith(1, "@openai/codex/package.json");
      expect(resolvePackageJson).toHaveBeenNthCalledWith(
        2,
        "@openai/codex-linux-x64/package.json",
        "/repo/node_modules/@openai/codex/package.json",
      );
    });

    it("throws a distinct unsupported-platform error for unsupported targets", async () => {
      const { _findCodexPathForTesting } = await import("../src/exec");

      expect(() =>
        _findCodexPathForTesting({
          envPath: "/usr/local/bin:/opt/homebrew/bin",
          platform: "freebsd",
          arch: "x64",
          pathExists: () => false,
          resolvePackageJson: () => {
            throw new Error("package lookup should not be used for unsupported targets");
          },
        }),
      ).toThrow(/unsupported.*freebsd.*x64/i);
    });
  });

  it("rejects when exit happens before stdout closes", async () => {
    const { CodexExec } = await import("../src/exec");
    const child = createEarlyExitChild();
    spawnMock.mockReturnValue(child as unknown as child_process.ChildProcess);

    const exec = new CodexExec("codex");
    const runPromise = (async () => {
      for await (const _ of exec.run({ input: "hi" })) {
        // no-op
      }
    })().then(
      () => ({ status: "resolved" as const }),
      (error) => ({ status: "rejected" as const, error }),
    );

    const result = await Promise.race([
      runPromise,
      delay(500).then(() => ({ status: "timeout" as const })),
    ]);

    expect(result.status).toBe("rejected");
    if (result.status === "rejected") {
      expect(result.error).toBeInstanceOf(Error);
      expect(result.error.message).toMatch(/Codex Exec exited/);
    }
  });

  it("places resume args before image args", async () => {
    const { CodexExec } = await import("../src/exec");
    spawnMock.mockClear();
    const child = new FakeChildProcess();
    spawnMock.mockReturnValue(child as unknown as child_process.ChildProcess);

    setImmediate(() => {
      child.stdout.end();
      child.stderr.end();
      child.emit("exit", 0, null);
    });

    const exec = new CodexExec("codex");
    for await (const _ of exec.run({ input: "hi", images: ["img.png"], threadId: "thread-id" })) {
      // no-op
    }

    const commandArgs = spawnMock.mock.calls[0]?.[1] as string[] | undefined;
    expect(commandArgs).toBeDefined();
    const resumeIndex = commandArgs!.indexOf("resume");
    const imageIndex = commandArgs!.indexOf("--image");
    expect(resumeIndex).toBeGreaterThan(-1);
    expect(imageIndex).toBeGreaterThan(-1);
    expect(resumeIndex).toBeLessThan(imageIndex);
  });

  it("allows overriding the env passed to the Codex CLI", async () => {
    const { CodexExec } = await import("../src/exec");
    spawnMock.mockClear();
    const child = new FakeChildProcess();
    spawnMock.mockReturnValue(child as unknown as child_process.ChildProcess);

    setImmediate(() => {
      child.stdout.end();
      child.stderr.end();
      child.emit("exit", 0, null);
    });

    process.env.CODEX_ENV_SHOULD_NOT_LEAK = "leak";

    try {
      const exec = new CodexExec("codex", {
        CODEX_HOME: "/tmp/codex-home",
        CUSTOM_ENV: "custom",
      });

      for await (const _ of exec.run({
        input: "custom env",
        apiKey: "test",
        baseUrl: "https://example.test",
      })) {
        // no-op
      }

      const commandArgs = spawnMock.mock.calls[0]?.[1] as string[] | undefined;
      expect(commandArgs).toBeDefined();
      const spawnOptions = spawnMock.mock.calls[0]?.[2] as child_process.SpawnOptions | undefined;
      const spawnEnv = spawnOptions?.env as Record<string, string> | undefined;
      expect(spawnEnv).toBeDefined();
      if (!spawnEnv || !commandArgs) {
        throw new Error("Spawn args missing");
      }

      expect(spawnEnv.CODEX_HOME).toBe("/tmp/codex-home");
      expect(spawnEnv.CUSTOM_ENV).toBe("custom");
      expect(spawnEnv.CODEX_ENV_SHOULD_NOT_LEAK).toBeUndefined();
      expect(spawnEnv.CODEX_API_KEY).toBe("test");
      expect(spawnEnv.CODEX_INTERNAL_ORIGINATOR_OVERRIDE).toBeDefined();
      expect(commandArgs).toContain("--config");
      expect(commandArgs).toContain(`openai_base_url=${JSON.stringify("https://example.test")}`);
    } finally {
      delete process.env.CODEX_ENV_SHOULD_NOT_LEAK;
    }
  });

  it("uses powershell.exe for Windows wrapper paths", async () => {
    const { CodexExec } = await import("../src/exec");
    spawnMock.mockClear();
    const child = new FakeChildProcess();
    spawnMock.mockReturnValue(child as unknown as child_process.ChildProcess);

    setImmediate(() => {
      child.stdout.end();
      child.stderr.end();
      child.emit("exit", 0, null);
    });

    await withMockedPlatform("win32", async () => {
      const exec = new CodexExec(
        "C:\\Users\\me\\AppData\\Local\\Programs\\Mcodex\\bin\\mcodex.ps1",
      );
      for await (const _ of exec.run({ input: "hello world" })) {
        // no-op
      }
    });

    expect(spawnMock).toHaveBeenCalledTimes(1);
    expect(spawnMock).toHaveBeenCalledWith(
      "powershell.exe",
      [
        "-NoLogo",
        "-NoProfile",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        "C:\\Users\\me\\AppData\\Local\\Programs\\Mcodex\\bin\\mcodex.ps1",
        "exec",
        "--experimental-json",
      ],
      expect.objectContaining({
        signal: undefined,
      }),
    );
  });
});
