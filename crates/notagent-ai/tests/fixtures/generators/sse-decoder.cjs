// Verbatim from packages/ai/src/api/anthropic-messages.ts (types stripped).
function flushSseEvent(state) {
  if (!state.event && state.data.length === 0) return null;
  const event = { event: state.event, data: state.data.join("\n"), raw: [...state.raw] };
  state.event = null; state.data = []; state.raw = [];
  return event;
}
function decodeSseLine(line, state) {
  if (line === "") return flushSseEvent(state);
  state.raw.push(line);
  if (line.startsWith(":")) return null;
  const delimiterIndex = line.indexOf(":");
  const fieldName = delimiterIndex === -1 ? line : line.slice(0, delimiterIndex);
  let value = delimiterIndex === -1 ? "" : line.slice(delimiterIndex + 1);
  if (value.startsWith(" ")) value = value.slice(1);
  if (fieldName === "event") state.event = value;
  else if (fieldName === "data") state.data.push(value);
  return null;
}
function nextLineBreakIndex(text) {
  const cr = text.indexOf("\r"), nl = text.indexOf("\n");
  if (cr === -1) return nl;
  if (nl === -1) return cr;
  return Math.min(cr, nl);
}
function consumeLine(text) {
  const i = nextLineBreakIndex(text);
  if (i === -1) return null;
  let next = i + 1;
  if (text[i] === "\r" && text[next] === "\n") next += 1;
  return { line: text.slice(0, i), rest: text.slice(next) };
}
function decodeAll(chunks) {
  const state = { event: null, data: [], raw: [] };
  let buffer = "";
  const out = [];
  for (const chunk of chunks) {
    buffer += chunk;
    let consumed = consumeLine(buffer);
    while (consumed) {
      buffer = consumed.rest;
      const e = decodeSseLine(consumed.line, state);
      if (e) out.push(e);
      consumed = consumeLine(buffer);
    }
  }
  let consumed = consumeLine(buffer);
  while (consumed) {
    buffer = consumed.rest;
    const e = decodeSseLine(consumed.line, state);
    if (e) out.push(e);
    consumed = consumeLine(buffer);
  }
  if (buffer.length > 0) { const e = decodeSseLine(buffer, state); if (e) out.push(e); }
  const trailing = flushSseEvent(state);
  if (trailing) out.push(trailing);
  return out;
}

const payloads = [
  "event: message_start\ndata: {\"a\":1}\n\n",
  "data: one\ndata: two\n\n",
  ": comment\ndata: v\n\n",
  ": only-comment\n\n",
  "data:  leading-space\n\n",
  "data\nevent\n\n",
  "event: done\ndata: {}",
  "event: a\rdata: b\r\r",
  "event: a\r\ndata: b\r\n\r\n",
  "\n\n\n",
  "event: e1\ndata: d1\n\nevent: e2\ndata: d2\n\n",
  "id: 1\nretry: 500\nevent: e\ndata: d\n\n",
  "data: {\"text\":\"h\\u00e9llo\"}\n\n",
  "event: error\ndata: {\"type\":\"error\"}\n\n",
  "data: trailing-no-newline",
  "",
];
const out = [];
for (const payload of payloads) {
  // whole, plus every split point
  const variants = [[payload]];
  for (let i = 1; i < payload.length; i++) variants.push([payload.slice(0, i), payload.slice(i)]);
  for (const chunks of variants) {
    out.push(JSON.stringify({ chunks, events: decodeAll(chunks) }));
  }
}
process.stdout.write(out.join("\n") + "\n");
