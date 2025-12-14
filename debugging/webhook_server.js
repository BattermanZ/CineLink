// Lightweight webhook listener for debugging Notion (or other) webhook payloads.
// Run with: node debugging/webhook_server.js

const http = require("http");
const fs = require("fs");
const path = require("path");

const PORT = 3146;
const LOG_PATH = path.join(__dirname, "webhook_logs.jsonl");

const server = http.createServer((req, res) => {
  const chunks = [];

  req.on("data", (chunk) => chunks.push(chunk));

  req.on("end", () => {
    const rawBody = Buffer.concat(chunks).toString("utf8");
    const timestamp = new Date().toISOString();

    let parsed;
    try {
      parsed = JSON.parse(rawBody);
    } catch {
      parsed = null;
    }

    console.log("---- webhook received ----");
    console.log("time:", timestamp);
    console.log("method:", req.method);
    console.log("url:", req.url);
    console.log("headers:", req.headers);
    console.log("raw body:", rawBody);
    if (parsed) {
      console.log("json body:", parsed);
    }
    console.log("--------------------------\n");

    const logEntry = {
      timestamp,
      method: req.method,
      url: req.url,
      headers: req.headers,
      rawBody,
      jsonBody: parsed,
    };

    fs.appendFile(LOG_PATH, `${JSON.stringify(logEntry)}\n`, (err) => {
      if (err) {
        console.error("Failed to write log entry:", err);
      }
    });

    res.writeHead(200, { "Content-Type": "application/json" });
    res.end(JSON.stringify({ status: "ok", received_at: timestamp }));
  });

  req.on("error", (err) => {
    console.error("Request error:", err);
  });
});

server.listen(PORT, () => {
  console.log(`Debug webhook server listening on http://0.0.0.0:${PORT}`);
});
