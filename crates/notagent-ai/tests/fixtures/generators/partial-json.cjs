const { parse: partialParse } = require("/Users/dev/projects/notagent-main/node_modules/partial-json");

// Verbatim from packages/ai/src/utils/json-parse.ts (types stripped).
const VALID_JSON_ESCAPES = new Set(['"', "\\", "/", "b", "f", "n", "r", "t", "u"]);
function isControlCharacter(char) { const cp = char.codePointAt(0); return cp !== undefined && cp >= 0x00 && cp <= 0x1f; }
function escapeControlCharacter(char) {
  switch (char) {
    case "\b": return "\\b"; case "\f": return "\\f"; case "\n": return "\\n";
    case "\r": return "\\r"; case "\t": return "\\t";
    default: return `\\u${char.codePointAt(0)?.toString(16).padStart(4, "0") ?? "0000"}`;
  }
}
function repairJson(json) {
  let repaired = ""; let inString = false;
  for (let index = 0; index < json.length; index++) {
    const char = json[index];
    if (!inString) { repaired += char; if (char === '"') inString = true; continue; }
    if (char === '"') { repaired += char; inString = false; continue; }
    if (char === "\\") {
      const nextChar = json[index + 1];
      if (nextChar === undefined) { repaired += "\\\\"; continue; }
      if (nextChar === "u") {
        const unicodeDigits = json.slice(index + 2, index + 6);
        if (/^[0-9a-fA-F]{4}$/.test(unicodeDigits)) { repaired += `\\u${unicodeDigits}`; index += 5; continue; }
      }
      if (VALID_JSON_ESCAPES.has(nextChar)) { repaired += `\\${nextChar}`; index += 1; continue; }
      repaired += "\\\\"; continue;
    }
    repaired += isControlCharacter(char) ? escapeControlCharacter(char) : char;
  }
  return repaired;
}
function parseJsonWithRepair(json) {
  try { return JSON.parse(json); }
  catch (error) { const r = repairJson(json); if (r !== json) return JSON.parse(r); throw error; }
}
function parseStreamingJson(partialJson) {
  if (!partialJson || partialJson.trim() === "") return {};
  try { return parseJsonWithRepair(partialJson); }
  catch {
    try { const result = partialParse(partialJson); return result ?? {}; }
    catch {
      try { const result = partialParse(repairJson(partialJson)); return result ?? {}; }
      catch { return {}; }
    }
  }
}

const samples = [
  '{"command": "ls -la /tmp", "timeout": 5000}',
  '{"path": "/Users/x/a.ts", "content": "line1\\nline2\\t\\"quoted\\"", "mode": 420}',
  '{"items": [1, 2.5, -3, 1e3, true, false, null], "nested": {"a": {"b": [{"c": "d"}]}}}',
  '{"pattern": "\\\\d+\\\\s*", "flags": "gi", "unicode": "héllo 🙈 世界"}',
  '{"empty_obj": {}, "empty_arr": [], "empty_str": ""}',
  '{"a": "with \\u00e9 escape", "b": "trailing backslash test"}',
  '[{"id": 1, "tags": ["x", "y"]}, {"id": 2, "tags": []}]',
  '{"num": 12345678901234567890, "float": 0.000003, "exp": 1.5e-7}',
];
const inputs = new Set();
for (const sample of samples) {
  for (let i = 0; i <= sample.length; i++) inputs.add(sample.slice(0, i));
}
// Malformed / repair cases.
for (const extra of [
  '{"a": "raw\nnewline"}', '{"a": "tab\there"}', '{"a": "bad \\q escape"}',
  '{"a": "trailing \\', '{"a": "bad \\u12 short"}', '{"a": 1,}', '{"a": 1, }',
  '  ', '', 'null', 'true', 'fal', 'nu', 'NaN', 'Infinity', '-Infinity', '-Inf',
  '-', '123', '12.', '1e', '1e5', '"just a string"', '"unterminated',
  '"escaped \\"inner\\" done"', '[1, 2,', '{"a":', '{"a"', '{', '[', '}', ']',
  '{"a": [1, {"b": "c"', 'garbage', '{"a": tru', '{"a": nul',
]) inputs.add(extra);

// Rust strings are always valid UTF-8. Prefixes that cut an astral character in half
// produce an unpaired UTF-16 surrogate in JS; such an input cannot exist in Rust
// (the SSE decoder assembles complete UTF-8), so those cases are skipped.
function hasUnpairedSurrogate(text) {
  for (let i = 0; i < text.length; i++) {
    const code = text.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = text.charCodeAt(i + 1);
      if (!(next >= 0xdc00 && next <= 0xdfff)) return true;
      i++;
    } else if (code >= 0xdc00 && code <= 0xdfff) return true;
  }
  return false;
}

const out = [];
let skipped = 0;
for (const input of inputs) {
  if (hasUnpairedSurrogate(input)) { skipped++; continue; }
  let result;
  try { result = JSON.stringify(parseStreamingJson(input)); }
  catch (e) { result = "__THROWS__"; }
  if (result !== undefined && hasUnpairedSurrogate(result)) { skipped++; continue; }
  out.push(JSON.stringify({ input, expected: result === undefined ? "__UNDEFINED__" : result }));
}
process.stderr.write(`skipped ${skipped} inputs with unpaired surrogates\n`);
process.stdout.write(out.join("\n") + "\n");
