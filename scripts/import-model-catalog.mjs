#!/usr/bin/env node

import { createHash } from "node:crypto";
import {
	existsSync,
	mkdirSync,
	readFileSync,
	readdirSync,
	renameSync,
	unlinkSync,
	writeFileSync,
} from "node:fs";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PRESERVED_PROVIDERS = new Set(["cline-pass"]);
const EMBEDDED_DIR = join(ROOT, "crates/notagent-ai/data");
const PROVIDER_FIXTURE = join(ROOT, "crates/notagent-ai/tests/fixtures/providers.jsonl");
// Uploaded as is to the site root, where installed binaries fetch
// `/api/models/providers/<id>.json`. It lives in the repository so that the
// published catalog can never lag behind the embedded one unnoticed; the
// model_data tests compare the two.
const PUBLISHED_DIR = join(ROOT, "api/models");

function usage() {
	throw new Error(
		"Usage: node scripts/import-model-catalog.mjs [<flat-provider-directory>]\n" +
			"Without a directory the published catalog is regenerated from the embedded data.",
	);
}

function sortedObject(entries) {
	return Object.fromEntries(Array.from(entries).sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0)));
}

function sha256(value) {
	return createHash("sha256").update(value).digest("hex");
}

function readJsonObject(path, label) {
	const value = JSON.parse(readFileSync(path, "utf8"));
	if (typeof value !== "object" || value === null || Array.isArray(value)) {
		throw new Error(`${label} must contain a JSON object`);
	}
	return value;
}

function providerFiles(directory) {
	return readdirSync(directory)
		.filter((file) => file.endsWith(".json") && file !== "manifest.json" && file !== "image-models.json")
		.sort();
}

function assertSameFiles(expected, actual) {
	if (expected.length === actual.length && expected.every((file, index) => file === actual[index])) return;
	const expectedSet = new Set(expected);
	const actualSet = new Set(actual);
	const missing = expected.filter((file) => !actualSet.has(file));
	const extra = actual.filter((file) => !expectedSet.has(file));
	throw new Error(
		`Provider files differ; update the Rust provider registry deliberately first. Missing: ${missing.join(", ") || "none"}. Extra: ${extra.join(", ") || "none"}.`,
	);
}

function validateModel(provider, modelId, model) {
	const label = `${provider}/${modelId}`;
	if (typeof model !== "object" || model === null || Array.isArray(model)) {
		throw new Error(`${label} must be an object`);
	}
	if (model.id !== modelId) throw new Error(`${label} has a mismatched id`);
	if (model.provider !== provider) throw new Error(`${label} has a mismatched provider`);
	if (typeof model.api !== "string" || model.api.length === 0) throw new Error(`${label} has no api`);
	if (typeof model.name !== "string" || model.name.length === 0) throw new Error(`${label} has no name`);
	if (typeof model.baseUrl !== "string") throw new Error(`${label} has no baseUrl`);
	if (typeof model.contextWindow !== "number" || model.contextWindow <= 0) {
		throw new Error(`${label} has an invalid contextWindow`);
	}
	if (typeof model.maxTokens !== "number" || model.maxTokens <= 0) {
		throw new Error(`${label} has invalid maxTokens`);
	}
}

function flattenGroups(provider, groups) {
	const models = new Map();
	for (const entries of Object.values(groups)) {
		for (const [modelId, model] of Object.entries(entries)) {
			if (models.has(modelId)) throw new Error(`${provider}/${modelId} appears in more than one API group`);
			validateModel(provider, modelId, model);
			models.set(modelId, model);
		}
	}
	return sortedObject(models);
}

function groupModels(provider, models) {
	const groups = new Map();
	for (const [modelId, model] of Object.entries(models).sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))) {
		validateModel(provider, modelId, model);
		const entries = groups.get(model.api) ?? [];
		entries.push([modelId, model]);
		groups.set(model.api, entries);
	}
	if (groups.size === 0) throw new Error(`${provider} contains no models`);
	return sortedObject(
		Array.from(groups, ([api, entries]) => [api, sortedObject(entries)]),
	);
}

function writeAtomic(path, content) {
	const temporary = `${path}.tmp`;
	writeFileSync(temporary, content);
	renameSync(temporary, path);
}

function updateProviderFixture(groupedContents) {
	const lines = readFileSync(PROVIDER_FIXTURE, "utf8")
		.split("\n")
		.filter((line) => line.trim().length > 0);
	const updated = lines.map((line) => {
		const provider = JSON.parse(line);
		const content = groupedContents.get(`${provider.id}.json`);
		if (!content && provider.dynamic === true && provider.modelCount === 0) return JSON.stringify(provider);
		if (!content) throw new Error(`Provider fixture references unknown provider: ${provider.id}`);
		const groups = JSON.parse(content);
		const modelIds = Object.values(groups).flatMap((models) => Object.keys(models));
		return JSON.stringify({ ...provider, modelCount: modelIds.length, modelIds });
	});
	writeAtomic(PROVIDER_FIXTURE, `${updated.join("\n")}\n`);
}

if (process.argv.length > 3) usage();
const sourceArgument = process.argv[2];
const sourceDir = sourceArgument === undefined ? undefined : resolve(sourceArgument);
if (sourceDir !== undefined && !existsSync(sourceDir)) {
	throw new Error(`Source directory does not exist: ${sourceDir}`);
}

const existingFiles = providerFiles(EMBEDDED_DIR);
const preservedFiles = existingFiles.filter(
	(file) => sourceDir === undefined || PRESERVED_PROVIDERS.has(basename(file, ".json")),
);
const generatedFiles = existingFiles.filter((file) => !PRESERVED_PROVIDERS.has(basename(file, ".json")));
const multilineFiles = new Set(
	generatedFiles.filter((file) => readFileSync(join(EMBEDDED_DIR, file), "utf8").trimEnd().includes("\n")),
);
const sourceFiles =
	sourceDir === undefined
		? []
		: providerFiles(sourceDir).filter((file) => !PRESERVED_PROVIDERS.has(basename(file, ".json")));
if (sourceDir !== undefined) assertSameFiles(generatedFiles, sourceFiles);

const flatCatalog = new Map();
const groupedContents = new Map();
for (const file of sourceFiles) {
	const provider = basename(file, ".json");
	const models = readJsonObject(join(sourceDir, file), file);
	const groups = groupModels(provider, models);
	flatCatalog.set(provider, sortedObject(Object.entries(models)));
	groupedContents.set(file, `${JSON.stringify(groups, null, multilineFiles.has(file) ? "\t" : undefined)}\n`);
}
for (const file of preservedFiles) {
	const provider = basename(file, ".json");
	const path = join(EMBEDDED_DIR, file);
	const content = readFileSync(path, "utf8");
	const raw = readJsonObject(path, file);
	flatCatalog.set(provider, flattenGroups(provider, raw));
	groupedContents.set(file, content);
}

const structure = {};
for (const [provider, models] of Array.from(flatCatalog).sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))) {
	structure[provider] = sortedObject(Object.entries(models).map(([modelId, model]) => [modelId, model.api]));
}
const fileContents = sortedObject(groupedContents);
updateProviderFixture(groupedContents);
const structureHash = sha256(JSON.stringify(structure));
const fileHashes = sortedObject(Object.entries(fileContents).map(([file, content]) => [file, sha256(content)]));
// A binary only takes a remote model that was published after its own catalog
// was generated, so the timestamp moves only when the catalog does. Rerunning
// without a change must not make an older binary look current.
const previousManifestPath = join(EMBEDDED_DIR, "manifest.json");
const previousManifest = existsSync(previousManifestPath)
	? readJsonObject(previousManifestPath, "manifest.json")
	: undefined;
const unchanged =
	previousManifest?.structureHash === structureHash &&
	JSON.stringify(previousManifest?.files) === JSON.stringify(fileHashes);
const manifest = {
	schemaVersion: 3,
	generatedAt: unchanged ? previousManifest.generatedAt : new Date().toISOString(),
	structureHash,
	files: fileHashes,
};

for (const [file, content] of Object.entries(fileContents)) writeAtomic(join(EMBEDDED_DIR, file), content);
writeAtomic(previousManifestPath, `${JSON.stringify(manifest)}\n`);

const publishedProvidersDir = join(PUBLISHED_DIR, "providers");
mkdirSync(publishedProvidersDir, { recursive: true });
const publishedFiles = new Set();
for (const [provider, models] of Array.from(flatCatalog).sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))) {
	publishedFiles.add(`${provider}.json`);
	writeAtomic(join(publishedProvidersDir, `${provider}.json`), `${JSON.stringify(models, null, 2)}\n`);
}
for (const file of readdirSync(publishedProvidersDir)) {
	if (!publishedFiles.has(file)) unlinkSync(join(publishedProvidersDir, file));
}
const mergedCatalog = sortedObject(flatCatalog);
writeAtomic(join(PUBLISHED_DIR, "models.json"), `${JSON.stringify(mergedCatalog, null, 2)}\n`);
writeAtomic(join(PUBLISHED_DIR, "providers.json"), `${JSON.stringify(Object.keys(mergedCatalog), null, 2)}\n`);
writeAtomic(join(PUBLISHED_DIR, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

const modelCount = Object.values(mergedCatalog).reduce((count, models) => count + Object.keys(models).length, 0);
console.log(`Imported ${modelCount} models across ${Object.keys(mergedCatalog).length} providers.`);
