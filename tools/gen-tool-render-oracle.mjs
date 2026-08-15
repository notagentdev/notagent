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

// Set after the theme is loaded, which still reads the real package: both sides
// resolve the packaged docs against this, so the compact `read` header
// classifies the same paths in the generator and in the test.
process.env.NOTAGENT_PACKAGE_DIR = "/oracle-package";

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



// Cases whose output runs through the syntax highlighter compare their text
// without ANSI: highlight.js is substituted by tree-sitter (master plan, class
// 3), so the token colours differ by construction while the layout must not.
const PLAIN = "plain";

function todo(content, status) {
	return { content, activeForm: `${content}ing`, status };
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
	// ---------------------------------------------------------------- read
	{ tool: "read", args: { path: "src/index.ts" } },
	{ tool: "read", args: {} },
	{ tool: "read", args: { path: 5 } },
	{ tool: "read", args: { path: "src/index.ts", offset: 12 } },
	{ tool: "read", args: { path: "src/index.ts", offset: 12, limit: 30 } },
	{ tool: "read", args: { path: "src/index.ts", limit: 30 } },
	{ tool: "read", args: { path: "src/index.ts", offset: null, limit: null } },
	{ tool: "read", args: { path: "src/index.ts", offset: 3, limit: 1 } },
	{ tool: "read", args: { path: "{HOME}/notes.md" } },
	{ tool: "read", args: { file_path: "src/legacy.ts", path: "src/ignored.ts" } },
	// compact classifications: skill, packaged docs, agent instructions
	{ tool: "read", args: { path: "skills/review/SKILL.md" } },
	{ tool: "read", args: { path: "SKILL.md" } },
	{ tool: "read", args: { path: "/oracle-package/README.md" } },
	{ tool: "read", args: { path: "/oracle-package/docs/guide.md" } },
	{ tool: "read", args: { path: "/oracle-package/examples/basic.ts" } },
	{ tool: "read", args: { path: "/oracle-package/src/other.ts" } },
	{ tool: "read", args: { path: "AGENTS.md" } },
	{ tool: "read", args: { path: "CLAUDE.md", offset: 2 } },
	{ tool: "read", args: { path: "AGENTS.md" }, expanded: true },
	{
		tool: "read",
		args: { path: "src/index.ts" },
		result: { content: [text(lines(4, "code"))], details: undefined },
	},
	{
		tool: "read",
		args: { path: "src/index.ts" },
		expanded: true,
		compare: PLAIN,
		result: { content: [text("const x = 1;\nconst y = 2;\n\n")], details: undefined },
	},
	{
		tool: "read",
		args: { path: "notes.txt" },
		expanded: true,
		result: { content: [text(lines(20, "plain"))], details: undefined },
	},
	{
		tool: "read",
		args: { path: "notes.txt" },
		isError: true,
		result: { content: [text("Could not read file")], details: undefined },
	},
	{
		tool: "read",
		args: { path: "src/index.ts" },
		expanded: true,
		compare: PLAIN,
		result: { content: [text("const x = 1;")], details: { truncation: TRUNCATION.lines } },
	},
	{
		tool: "read",
		args: { path: "src/index.ts" },
		expanded: true,
		compare: PLAIN,
		result: { content: [text("const x = 1;")], details: { truncation: TRUNCATION.bytes } },
	},
	{
		tool: "read",
		args: { path: "src/index.ts" },
		expanded: true,
		compare: PLAIN,
		result: { content: [text("const x = 1;")], details: { truncation: TRUNCATION.firstLine } },
	},
	{
		tool: "read",
		args: { path: "photo.png" },
		expanded: true,
		result: {
			content: [text("Read image file [image/png]"), { type: "image", data: "AAAA", mimeType: "image/png" }],
			details: undefined,
		},
	},
	// -------------------------------------------------------- read_minified
	{ tool: "read_minified", args: { path: "src/index.ts" } },
	{ tool: "read_minified", args: { path: "src/index.ts", keep_comments: true } },
	{ tool: "read_minified", args: { path: "src/index.ts", keep_comments: false } },
	{ tool: "read_minified", args: { path: "src/index.ts", offset: 4, limit: 10, keep_comments: 1 } },
	{ tool: "read_minified", args: {} },
	{
		tool: "read_minified",
		args: { path: "src/index.ts" },
		expanded: true,
		compare: PLAIN,
		result: { content: [text(lines(14, "const a" + " = ")) ], details: { minified: true } },
	},
	{
		tool: "read_minified",
		args: { path: "src/index.ts" },
		expanded: true,
		compare: PLAIN,
		result: { content: [text("const x = 1;")], details: { minified: false } },
	},
	{
		tool: "read_minified",
		args: { path: "notes.txt" },
		result: { content: [text("plain")], details: { minified: false } },
	},
	// --------------------------------------------------------------- skill
	{ tool: "skill", args: { name: "code-review" } },
	{ tool: "skill", args: {} },
	{ tool: "skill", args: { name: 3 } },
	{
		tool: "skill",
		args: { name: "code-review" },
		result: { content: [text("envelope")], details: { name: "code-review", tokens: 1234, resources: [] } },
	},
	{
		tool: "skill",
		args: { name: "code-review" },
		result: {
			content: [text("envelope")],
			details: { name: "code-review", tokens: 12, resources: ["a.md", "b.md"] },
		},
	},
	{ tool: "skill", args: { name: "x" }, result: { content: [text("envelope")], details: undefined } },
	// ---------------------------------------------------------------- task
	{ tool: "task", args: { mode: "review", tasks: [{ prompt: "a" }] } },
	{ tool: "task", args: { mode: "review", tasks: [{ prompt: "a" }, { prompt: "b" }] } },
	{ tool: "task", args: { mode: "review", tasks: [], run_in_background: true } },
	{ tool: "task", args: {} },
	{ tool: "task", args: { mode: "review", tasks: [{}], run_in_background: "yes" } },
	{
		tool: "task",
		args: { mode: "review", tasks: [{}] },
		result: {
			content: [text("<subagent/>")],
			details: { mode: "review", results: [{ sessionId: "s1", failed: false, background: false }] },
		},
	},
	{
		tool: "task",
		args: { mode: "review", tasks: [{}, {}] },
		result: {
			content: [text("<subagent/>")],
			details: {
				mode: "review",
				results: [
					{ sessionId: "s1", failed: true, background: false },
					{ sessionId: "s2", failed: false, background: false },
				],
			},
		},
	},
	{
		tool: "task",
		args: { mode: "review", tasks: [{}, {}] },
		result: {
			content: [text("<subagent/>")],
			details: {
				mode: "review",
				results: [
					{ sessionId: "s1", failed: false, taskId: "t1", background: true },
					{ sessionId: "s2", failed: false, taskId: "t2", background: true },
				],
			},
		},
	},
	{ tool: "task", args: { mode: "review" }, result: { content: [text("x")], details: undefined } },
	// ----------------------------------------------------------- task tools
	{ tool: "task_list", args: {} },
	{ tool: "task_list", args: { all: true } },
	{ tool: "task_list", args: { all: "true" } },
	{ tool: "task_list", args: {}, result: { content: [text("x")], details: { count: 3, all: false } } },
	{ tool: "task_list", args: {}, result: { content: [text("x")], details: { count: 0, all: true } } },
	{ tool: "task_list", args: {}, result: { content: [text("x")], details: undefined } },
	{ tool: "task_output", args: { task_id: "task-7" } },
	{ tool: "task_output", args: {} },
	{
		tool: "task_output",
		args: { task_id: "task-7" },
		result: { content: [text("x")], details: { taskId: "task-7", status: "running", truncated: false } },
	},
	{ tool: "task_output", args: { task_id: "task-7" }, result: { content: [text("x")], details: undefined } },
	{ tool: "task_stop", args: { task_id: "task-7", reason: "done" } },
	{ tool: "task_stop", args: { task_id: 9 } },
	{
		tool: "task_stop",
		args: { task_id: "task-7" },
		result: { content: [text("x")], details: { taskId: "task-7", status: "stopped" } },
	},
	// ----------------------------------------------------------- todo_write
	{ tool: "todo_write", args: { todos: [] } },
	{ tool: "todo_write", args: { todos: [todo("Run tests", "pending")] } },
	{
		tool: "todo_write",
		args: { todos: [todo("Run tests", "pending"), todo("Ship it", "pending")] },
	},
	{ tool: "todo_write", args: {} },
	{
		tool: "todo_write",
		args: { todos: [todo("Run tests", "completed")] },
		result: {
			content: [text("<todos_updated/>")],
			details: { before: [todo("Run tests", "in_progress")], after: [todo("Run tests", "completed")] },
		},
	},
	{
		tool: "todo_write",
		args: { todos: [] },
		result: {
			content: [text("<todos_updated/>")],
			details: {
				before: [todo("Run tests", "completed"), todo("Ship it", "pending")],
				after: [],
			},
		},
	},
	{
		tool: "todo_write",
		args: { todos: [] },
		result: {
			content: [text("<todos_updated/>")],
			details: {
				before: [todo("Keep me", "pending")],
				after: [todo("Keep me", "pending"), todo("New one", "in_progress")],
			},
		},
	},
	{
		tool: "todo_write",
		args: { todos: [] },
		result: { content: [text("x")], details: { before: [], after: [] } },
	},
	{ tool: "todo_write", args: { todos: [] }, result: { content: [text("x")], details: undefined } },
	// --------------------------------------------------------------- write
	{ tool: "write", args: { path: "src/new.ts", content: "const x = 1;\n" }, compare: PLAIN },
	{ tool: "write", args: { path: "notes.txt", content: "plain text\nsecond line\n" } },
	{ tool: "write", args: { path: "src/new.ts" } },
	{ tool: "write", args: { path: "src/new.ts", content: 42 } },
	{ tool: "write", args: { path: "src/new.ts", content: "" } },
	{ tool: "write", args: { path: "src/big.ts", content: lines(24, "const line") }, compare: PLAIN },
	{
		tool: "write",
		args: { path: "src/big.ts", content: lines(24, "const line") },
		expanded: true,
		compare: PLAIN,
	},
	{ tool: "write", args: { path: "notes.txt", content: lines(24, "line") } },
	{ tool: "write", args: { content: "orphan content" } },
	{
		// The streaming path: the highlight cache is extended per chunk and the
		// first lines are re-highlighted, then the complete argument rebuilds it.
		tool: "write",
		compare: PLAIN,
		steps: [
			{ args: { path: "src/stream.ts", content: "const a" }, argsComplete: false },
			{ args: { path: "src/stream.ts", content: "const a = 1;\nconst b" }, argsComplete: false },
			{ args: { path: "src/stream.ts", content: "const a = 1;\nconst b = 2;\n" }, argsComplete: false },
			{ args: { path: "src/stream.ts", content: "const a = 1;\nconst b = 2;\n" }, argsComplete: true },
		],
	},
	{
		// A rewritten prefix drops the cache and highlights from scratch.
		tool: "write",
		compare: PLAIN,
		steps: [
			{ args: { path: "src/stream.ts", content: "const a = 1;" }, argsComplete: false },
			{ args: { path: "src/stream.ts", content: "let b = 2;" }, argsComplete: false },
		],
	},
	{
		// A changed path rebuilds the cache with the new language.
		tool: "write",
		compare: PLAIN,
		steps: [
			{ args: { path: "src/stream.ts", content: "const a = 1;" }, argsComplete: false },
			{ args: { path: "src/stream.py", content: "const a = 1;" }, argsComplete: false },
		],
	},
	{
		// Tabs and carriage returns are normalised while streaming.
		tool: "write",
		compare: PLAIN,
		steps: [
			{ args: { path: "src/tabs.ts", content: "if (x) {\r\n\tconst a" }, argsComplete: false },
			{ args: { path: "src/tabs.ts", content: "if (x) {\r\n\tconst a = 1;\r\n}" }, argsComplete: false },
		],
	},
	{
		tool: "write",
		args: { path: "src/new.ts", content: "const x = 1;" },
		compare: PLAIN,
		result: { content: [text("Successfully wrote 12 bytes to src/new.ts")], details: undefined },
	},
	{
		tool: "write",
		args: { path: "src/new.ts", content: "const x = 1;" },
		isError: true,
		compare: PLAIN,
		result: { content: [text("EACCES: permission denied")], details: undefined },
	},
	{
		tool: "write",
		args: { path: "src/new.ts", content: "const x = 1;" },
		isError: true,
		compare: PLAIN,
		result: { content: [{ type: "image", data: "AAAA", mimeType: "image/png" }], details: undefined },
	},
	// ------------------------------------------------------ patch_minified
	{ tool: "patch_minified", args: { path: "src/index.ts" } },
	{ tool: "patch_minified", args: {} },
	{ tool: "multi_patch_minified", args: { path: "src/index.ts" } },
	{
		tool: "patch_minified",
		args: { path: "src/index.ts" },
		result: {
			content: [text("ok")],
			details: { diff: "-  1 const x = 1;\n+  1 const y = 2;", patch: "", firstChangedLine: 1, warnings: [] },
		},
	},
	{
		tool: "patch_minified",
		args: { path: "src/index.ts" },
		result: {
			content: [text("ok")],
			details: { diff: "", patch: "", warnings: ["indentation was normalised"] },
		},
	},
	{
		tool: "multi_patch_minified",
		args: { path: "src/index.ts" },
		result: {
			content: [text("ok")],
			details: {
				diff: "-  1 a\n+  1 b",
				patch: "",
				warnings: ["first warning", "second warning"],
			},
		},
	},
	{
		tool: "patch_minified",
		args: { path: "src/index.ts" },
		isError: true,
		result: { content: [text("failed")], details: { diff: "-  1 a\n+  1 b", warnings: [] } },
	},
	{ tool: "patch_minified", args: { path: "src/index.ts" }, result: { content: [text("ok")], details: undefined } },
];


const definitions = createAllToolDefinitions(CWD);

const output = CASES.map((testCase) => {
	const definition = definitions[testCase.tool];
	if (!definition) throw new Error(`unknown tool ${testCase.tool}`);
	const width = testCase.width ?? 100;
	const result = testCase.result ? expand(testCase.result) : undefined;
	const state = {};
	const cwd = testCase.cwd ?? CWD;
	// A case either renders its arguments once or streams them in steps
	// through the same render state, the way tool-execution does.
	const steps = (testCase.steps ?? [{ args: testCase.args, argsComplete: testCase.argsComplete }]).map(
		(step) => ({
			args: expand(step.args),
			argsComplete: step.argsComplete ?? true,
		}),
	);

	const baseContext = {
		toolCallId: "oracle-call",
		invalidate: () => {},
		state,
		cwd,
		executionStarted: testCase.executionStarted ?? true,
		isPartial: testCase.isPartial ?? false,
		expanded: testCase.expanded ?? false,
		showImages: testCase.showImages ?? true,
		isError: testCase.isError ?? false,
	};

	let callComponent;
	const callLineSteps = steps.map((step) => {
		callComponent = definition.renderCall(step.args, theme, {
			...baseContext,
			args: step.args,
			argsComplete: step.argsComplete,
			lastComponent: callComponent,
		});
		return callComponent.render(width);
	});

	const lastStep = steps[steps.length - 1];
	let resultLines;
	if (result) {
		const resultComponent = definition.renderResult(
			result,
			{ expanded: baseContext.expanded, isPartial: baseContext.isPartial },
			theme,
			{ ...baseContext, args: lastStep.args, argsComplete: lastStep.argsComplete, lastComponent: undefined },
		);
		resultLines = resultComponent.render(width);
	}

	return {
		tool: testCase.tool,
		compare: testCase.compare ?? "bytes",
		steps: (testCase.steps ?? [{ args: testCase.args, argsComplete: testCase.argsComplete }]).map((step) => ({
			args: step.args,
			argsComplete: step.argsComplete ?? true,
		})),
		cwd,
		width,
		expanded: baseContext.expanded,
		isPartial: baseContext.isPartial,
		isError: baseContext.isError,
		showImages: baseContext.showImages,
		executionStarted: baseContext.executionStarted,
		result: testCase.result ?? null,
		callLineSteps,
		resultLines: resultLines ?? null,
	};
});

process.stdout.write(JSON.stringify(output, null, "\t"));
