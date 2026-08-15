// Generates the tool renderer oracle for crates/notagent/tests/tool_render_oracle.rs.
//
// Every case runs one built-in tool's renderCall and, when the case carries a
// result, its renderResult — through the same shared render state, exactly as
// ToolExecutionComponent does. The rendered lines are recorded verbatim.
//
// Run from packages/coding-agent of the TypeScript repo:
//   FORCE_COLOR=1 npx tsx tools/gen-tool-render-oracle.mjs > .../tests/fixtures/tool-render-oracle.json
import * as os from "node:os";

const { setCapabilities } = await import("@notagent/tui");
setCapabilities({ images: null, trueColor: true, hyperlinks: false });

const { initTheme, theme } = await import("./src/modes/interactive/theme/theme.ts");
const { createAllToolDefinitions } = await import("./src/core/tools/index.ts");

initTheme("dark");

const CWD = "/oracle-cwd";
const HOME = os.homedir();

/** `{HOME}` in a fixture string is this machine's home directory. */
function expand(value) {
	if (typeof value === "string") return value.split("{HOME}").join(HOME);
	if (Array.isArray(value)) return value.map(expand);
	if (value && typeof value === "object") {
		return Object.fromEntries(Object.entries(value).map(([key, entry]) => [key, expand(entry)]));
	}
	return value;
}

function text(value) {
	return { type: "text", text: value };
}

const TRUNCATION = {
	lines: {
		content: "",
		truncated: true,
		truncatedBy: "lines",
		totalLines: 900,
		totalBytes: 40000,
		outputLines: 200,
		outputBytes: 9000,
		lastLinePartial: false,
		firstLineExceedsLimit: false,
		maxLines: 2000,
		maxBytes: 51200,
	},
	bytes: {
		content: "",
		truncated: true,
		truncatedBy: "bytes",
		totalLines: 900,
		totalBytes: 90000,
		outputLines: 120,
		outputBytes: 51200,
		lastLinePartial: false,
		firstLineExceedsLimit: false,
		maxLines: 2000,
		maxBytes: 51200,
	},
	firstLine: {
		content: "",
		truncated: true,
		truncatedBy: "bytes",
		totalLines: 1,
		totalBytes: 90000,
		outputLines: 1,
		outputBytes: 51200,
		lastLinePartial: false,
		firstLineExceedsLimit: true,
		maxLines: 2000,
		maxBytes: 51200,
	},
};

function lines(count, prefix = "line") {
	return Array.from({ length: count }, (_, index) => `${prefix} ${index + 1}`).join("\n");
}

const CASES = [
	// ---------------------------------------------------------------- grep
	{ tool: "grep", args: { pattern: "foo" } },
	{ tool: "grep", args: {} },
	{ tool: "grep", args: { pattern: "" } },
	{ tool: "grep", args: { pattern: 42 } },
	{ tool: "grep", args: { pattern: "foo", path: "src" } },
	{ tool: "grep", args: { pattern: "foo", path: "{HOME}/projects/app" } },
	{ tool: "grep", args: { pattern: "foo", path: null } },
	{ tool: "grep", args: { pattern: "foo", path: 7 } },
	{ tool: "grep", args: { pattern: "foo", glob: "*.ts", limit: 25 } },
	{ tool: "grep", args: { pattern: "foo", glob: "", limit: null } },
	{
		tool: "grep",
		args: { pattern: "foo" },
		result: { content: [text(lines(4, "match"))], details: undefined },
	},
	{
		tool: "grep",
		args: { pattern: "foo" },
		result: { content: [text(lines(30, "match"))], details: undefined },
	},
	{
		tool: "grep",
		args: { pattern: "foo" },
		expanded: true,
		result: { content: [text(lines(30, "match"))], details: undefined },
	},
	{
		tool: "grep",
		args: { pattern: "foo" },
		result: {
			content: [text("a match")],
			details: { matchLimitReached: 100, truncation: TRUNCATION.bytes, linesTruncated: true },
		},
	},
	{
		tool: "grep",
		args: { pattern: "foo" },
		result: { content: [text("")], details: { linesTruncated: true } },
	},
	// ------------------------------------------------------------------ ls
	{ tool: "ls", args: {} },
	{ tool: "ls", args: { path: "src" } },
	{ tool: "ls", args: { path: "" } },
	{ tool: "ls", args: { path: 3 } },
	{ tool: "ls", args: { path: "{HOME}/projects" } },
	{ tool: "ls", args: { path: "src", limit: 10 } },
	{
		tool: "ls",
		args: { path: "src" },
		result: { content: [text(lines(25, "entry"))], details: undefined },
	},
	{
		tool: "ls",
		args: { path: "src" },
		expanded: true,
		result: { content: [text(lines(25, "entry"))], details: undefined },
	},
	{
		tool: "ls",
		args: { path: "src" },
		result: {
			content: [text("entry")],
			details: { entryLimitReached: 200, truncation: TRUNCATION.lines },
		},
	},
	// ---------------------------------------------------------------- find
	{ tool: "find", args: { pattern: "*.ts" } },
	{ tool: "find", args: {} },
	{ tool: "find", args: { pattern: "*.ts", path: "src", limit: 5 } },
	{ tool: "find", args: { pattern: false, path: [] } },
	{
		tool: "find",
		args: { pattern: "*.ts" },
		result: { content: [text(lines(25, "src/file"))], details: undefined },
	},
	{
		tool: "find",
		args: { pattern: "*.ts" },
		result: {
			content: [text("src/a.ts")],
			details: { resultLimitReached: 500, truncation: TRUNCATION.bytes },
		},
	},
];

const definitions = createAllToolDefinitions(CWD);

const output = CASES.map((testCase) => {
	const definition = definitions[testCase.tool];
	if (!definition) throw new Error(`unknown tool ${testCase.tool}`);
	const width = testCase.width ?? 100;
	const args = expand(testCase.args);
	const result = testCase.result ? expand(testCase.result) : undefined;
	const state = {};
	let callComponent;
	let resultComponent;
	const context = {
		args,
		toolCallId: "oracle-call",
		invalidate: () => {},
		lastComponent: undefined,
		state,
		cwd: testCase.cwd ?? CWD,
		executionStarted: testCase.executionStarted ?? true,
		argsComplete: testCase.argsComplete ?? true,
		isPartial: testCase.isPartial ?? false,
		expanded: testCase.expanded ?? false,
		showImages: testCase.showImages ?? true,
		isError: testCase.isError ?? false,
	};

	callComponent = definition.renderCall(args, theme, { ...context, lastComponent: undefined });
	const callLines = callComponent.render(width);

	let resultLines;
	if (result) {
		resultComponent = definition.renderResult(
			result,
			{ expanded: context.expanded, isPartial: context.isPartial },
			theme,
			{ ...context, lastComponent: undefined },
		);
		resultLines = resultComponent.render(width);
	}

	return {
		tool: testCase.tool,
		args: testCase.args,
		cwd: testCase.cwd ?? CWD,
		width,
		expanded: context.expanded,
		isPartial: context.isPartial,
		isError: context.isError,
		showImages: context.showImages,
		argsComplete: context.argsComplete,
		executionStarted: context.executionStarted,
		result: testCase.result ?? null,
		callLines,
		resultLines: resultLines ?? null,
	};
});

process.stdout.write(JSON.stringify(output, null, "\t"));
