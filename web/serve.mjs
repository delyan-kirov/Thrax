// A tiny dependency-free static server for the playground (`web/site/`), for
// when you'd rather use Node than `python3 -m http.server`. Third-party
// servers (`http-server`) tend to send the wrong MIME for `.wasm`/ES modules
// or an aggressive cache; this sets the right content types and disables
// caching, so a rebuilt thrax.js/wasm is always picked up.
//
//   node web/serve.mjs           # http://localhost:8000
//   PORT=3000 node web/serve.mjs

import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize, sep } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("./site", import.meta.url));
const port = Number(process.env.PORT) || 8000;

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".ico": "image/x-icon",
};

createServer(async (req, res) => {
  let path = decodeURIComponent(new URL(req.url, "http://x").pathname);
  if (path === "/") path = "/index.html";

  const file = normalize(join(root, path));
  if (file !== root && !file.startsWith(root + sep)) {
    res.writeHead(403).end("forbidden"); // no path traversal out of web/site
    return;
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, {
      "content-type": MIME[extname(file)] || "application/octet-stream",
      "cache-control": "no-store",
    });
    res.end(body);
  } catch {
    res.writeHead(404).end("not found");
  }
}).listen(port, () =>
  console.log(`Thrax playground on http://localhost:${port}  (Ctrl-C to stop)`),
);
