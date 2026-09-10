import assert from "node:assert/strict";
import { test } from "node:test";
import worker from "../worker.js";

const noAssets = { ASSETS: { fetch() { throw new Error("Git requests must redirect"); } } };

test("redirects Git discovery to the repository while preserving query parameters", async () => {
  const response = await worker.fetch(
    new Request("https://ruddy.logan.md/info/refs?service=git-upload-pack"), noAssets,
  );
  assert.equal(response.status, 307);
  assert.equal(response.headers.get("location"),
    "https://github.com/logan-gatlin/ruddy.git/info/refs?service=git-upload-pack");
});

test("redirects the Git POST endpoint without changing its method", async () => {
  const response = await worker.fetch(
    new Request("https://ruddy.logan.md/git-upload-pack", { method: "POST", body: "git request" }), noAssets,
  );
  assert.equal(response.status, 307);
  assert.equal(response.headers.get("location"), "https://github.com/logan-gatlin/ruddy.git/git-upload-pack");
});

test("accepts Cargo's double-slash discovery path for a hostname-only remote", async () => {
  const response = await worker.fetch(
    new Request("https://ruddy.logan.md//info/refs?service=git-upload-pack"), noAssets,
  );
  assert.equal(response.status, 307);
  assert.equal(response.headers.get("location"),
    "https://github.com/logan-gatlin/ruddy.git/info/refs?service=git-upload-pack");
});

test("passes documentation, assets, and unknown paths to the static asset service", async () => {
  for (const path of ["/", "/download.html", "/dictionary.html", "/assets/style.css", "/missing", "/info/refs/other"]) {
    const request = new Request(`https://ruddy.logan.md${path}`);
    const expected = new Response("asset response", { status: path === "/missing" ? 404 : 200 });
    const response = await worker.fetch(request, {
      ASSETS: { fetch(actual) { assert.equal(actual, request); return expected; } },
    });
    assert.equal(response, expected);
  }
});
