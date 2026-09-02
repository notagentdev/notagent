#!/usr/bin/env node

import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync, renameSync, writeFileSync } from "node:fs";
import { basename, join, resolve } from "node:path";

const PRESERVED_PROVIDERS = new Set(["cline-pass"]);
const EMBEDDED_DIR = resolve("crates/notagent-ai/data");
const PROVIDER_FIXTURE = resolve("crates/notagent-ai/tests/fixtures/providers.jsonl");

function usage() {
	throw new Error(
		"Usage: node scripts/import-model-catalog.mjs <flat-provider-directory> <release-site-directory>",
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

const sourceArgument = process.argv[2];
const releaseArgument = process.argv[3];
if (!sourceArgument || !releaseArgument) usage();

const sourceDir = resolve(sourceArgument);
const releaseDir = resolve(releaseArgument);
if (!existsSync(sourceDir)) throw new Error(`Source directory does not exist: ${sourceDir}`);
if (!existsSync(releaseDir)) throw new Error(`Release directory does not exist: ${releaseDir}`);

const existingFiles = providerFiles(EMBEDDED_DIR);
const preservedFiles = existingFiles.filter((file) => PRESERVED_PROVIDERS.has(basename(file, ".json")));
const generatedFiles = existingFiles.filter((file) => !PRESERVED_PROVIDERS.has(basename(file, ".json")));
const multilineFiles = new Set(
	generatedFiles.filter((file) => readFileSync(join(EMBEDDED_DIR, file), "utf8").trimEnd().includes("\n")),
);
const sourceFiles = providerFiles(sourceDir).filter(
	(file) => !PRESERVED_PROVIDERS.has(basename(file, ".json")),
);
assertSameFiles(generatedFiles, sourceFiles);

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
const manifest = {
	schemaVersion: 3,
	generatedAt: new Date().toISOString(),
	structureHash: sha256(JSON.stringify(structure)),
	files: sortedObject(Object.entries(fileContents).map(([file, content]) => [file, sha256(content)])),
};

for (const [file, content] of Object.entries(fileContents)) writeAtomic(join(EMBEDDED_DIR, file), content);
writeAtomic(join(EMBEDDED_DIR, "manifest.json"), `${JSON.stringify(manifest)}\n`);

const releaseProvidersDir = join(releaseDir, "providers");
for (const [provider, models] of Array.from(flatCatalog).sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0))) {
	writeAtomic(join(releaseProvidersDir, `${provider}.json`), `${JSON.stringify(models, null, 2)}\n`);
}
const mergedCatalog = sortedObject(flatCatalog);
writeAtomic(join(releaseDir, "models.json"), `${JSON.stringify(mergedCatalog, null, 2)}\n`);
writeAtomic(join(releaseDir, "providers.json"), `${JSON.stringify(Object.keys(mergedCatalog), null, 2)}\n`);
writeAtomic(join(releaseDir, "manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

const modelCount = Object.values(mergedCatalog).reduce((count, models) => count + Object.keys(models).length, 0);
console.log(`Imported ${modelCount} models across ${Object.keys(mergedCatalog).length} providers.`);
