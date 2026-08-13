/**
 * Generates `tests/fixtures/stdin-buffer-oracle.json`: the synchronous event
 * stream of `packages/tui/src/stdin-buffer.ts` for realistic input streams cut
 * into every plausible chunking. Timeouts are never awaited, so only the
 * chunk-boundary state machine is captured.
 *
 * Usage (from crates/notagent-tui/):
 *   node tools/gen-stdin-buffer-oracle.mjs > tests/fixtures/stdin-buffer-oracle.json
 */
const TS_REPO = process.env.NOTAGENT_TS_REPO ?? "/Users/dev/projects/notagent-main";
const { StdinBuffer } = await import(`${TS_REPO}/packages/tui/src/stdin-buffer.ts`);

const STREAMS = [
	"a", "abc", "hello 世界",
	"\x1b[A", "\x1b[<35;20;5m", "\x1b[M abc", "\x1bOA", "\x1ba", "\x1b\x1b", "\x1b\x1b[27;1:3u",
	"\x1b[97u\x1b[97;1:3u", "\x1b[224uà", "\x1b[64u@", "\x1b[64;3u@", "\x1b[97ub",
	"abc\x1b[A", "\x1b[Aabc", "\x1b[A\x1b[B\x1b[C", "\x1b]0;title\x07x", "\x1b]8;;http://a\x1b\\y",
	"\x1bP>|term\x1b\\z", "\x1b_Gi=1;abc\x1b\\q", "\x1b[200~pasted\x1b[201~",
	"a\x1b[200~one\ntwo\x1b[201~b", "\x1b[200~\x1b[A inside\x1b[201~",
	"\x1b[1;5H\x1b[3;1:3~", "\x1b[<0;1;1M\x1b[<0;1;1m", "\x1b[Z\t\r\n",
	"\x1b[?7u", "\x1b[?62;4;52c", "\x1b[", "\x1b", "\x1b[<35",
	"\x1b[27;5;99~\x1b[27;2;69~", "\x1b[57417u\x1b[57426u",
	"\x1b[200~unterminated", "x\x1b[200~a\x1b[201~\x1b[200~b\x1b[201~y",
];

/** All ways to cut `s` into `n` chunks, capped to keep the fixture small. */
function chunkings(s, limit) {
	const out = [[s]];
	for (let i = 1; i < s.length; i++) out.push([s.slice(0, i), s.slice(i)]);
	for (let i = 1; i < s.length - 1 && out.length < limit; i++) {
		for (let j = i + 1; j < s.length && out.length < limit; j++) {
			out.push([s.slice(0, i), s.slice(i, j), s.slice(j)]);
		}
	}
	// Single characters as chunks (worst case fragmentation)
	out.push([...s]);
	return out.slice(0, limit);
}

const cases = [];
for (const stream of STREAMS) {
	for (const chunks of chunkings(stream, 60)) {
		const buffer = new StdinBuffer({ timeout: 10 });
		const events = [];
		buffer.on("data", (d) => events.push({ type: "data", data: d }));
		buffer.on("paste", (d) => events.push({ type: "paste", data: d }));
		for (const chunk of chunks) buffer.process(chunk);
		cases.push({ chunks, events, remainder: buffer.getBuffer() });
		buffer.destroy();
	}
}

process.stdout.write(JSON.stringify({ cases }, null, 0));
process.stderr.write(`cases: ${cases.length}\n`);
