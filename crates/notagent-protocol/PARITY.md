# Parity-Ledger: notagent-protocol

TS-Quelle: `/Users/dev/projects/notagent-main/packages/protocol` (1 236 LOC in src/) — Workstream C.

Regeln: Master-Plan `plans/2026-08-13-rust-port-master-v1.md`, Abschnitt "Drift-Kontrolle".
Format und Status-Werte: `CONVENTIONS.md`, Abschnitt "Parity-Ledger".

## Lektüre-Protokoll

| Datum | TS-/Referenz-Datei | LOC | gelesen von (Task) |
|---|---|---|---|
| 2026-08-13 | packages/protocol/src/index.ts | 4 | C-Task 2 |
| 2026-08-13 | packages/protocol/src/schemas.ts | 450 | C-Task 2 |
| 2026-08-13 | packages/protocol/src/codec.ts | 172 | C-Task 2 |
| 2026-08-13 | packages/protocol/src/framing.ts | 165 | C-Task 2 |
| 2026-08-13 | packages/protocol/src/cbor/index.ts | 9 | C-Task 2 |
| 2026-08-13 | packages/protocol/src/cbor/options.ts | 52 | C-Task 2 |
| 2026-08-13 | packages/protocol/src/cbor/encoder.ts | 216 | C-Task 2 |
| 2026-08-13 | packages/protocol/src/cbor/decoder.ts | 168 | C-Task 2 |
| 2026-08-13 | packages/protocol/test/cbor/cbor.test.ts | 175 | C-Task 2 |
| 2026-08-13 | packages/protocol/test/framing.test.ts | 117 | C-Task 2 |
| 2026-08-13 | packages/protocol/test/protocol.test.ts | 424 | C-Task 2 |
| 2026-08-13 | packages/protocol/package.json (Deps, Engines) | 60 | C-Task 2 |

Gelesen: 12 Dateien, 2 012 LOC — das gesamte Paket inklusive Tests.

## Ledger

| TS-Datei | LOC | Rust-Modul | Status | Abweichung (Klasse + Begründung) |
|---|---|---|---|---|
| src/index.ts | 4 | src/lib.rs | verifiziert | — (Re-Export-Barrel) |
| src/cbor/index.ts | 9 | src/cbor.rs | verifiziert | — (Re-Export-Barrel) |
| src/cbor/options.ts | 52 | src/cbor/options.rs | verifiziert | Klasse 1: `CborError` ist ein Enum mit den Varianten `Cbor` (TS `class CborError`) und `Range` (TS `RangeError` aus `resolveLimit`); die Prüfungen „Integer" und „≥ 0" entfallen, da Limits `u64` sind |
| src/cbor/encoder.ts | 216 | src/cbor/encoder.rs | verifiziert | Klasse 1: JS-`unknown` → `CborValue` (src/cbor/value.rs). Nicht darstellbar und daher entfallen: `undefined`, Array-Löcher, bigint/symbol/function/Date/Map, Symbol-Keys, Zyklen (`ancestors`-Set), verlustbehaftete Strings (Rust-`String` ist gültiges UTF-8). Alle übrigen Ablehnungsregeln 1:1 |
| src/cbor/decoder.ts | 168 | src/cbor/decoder.rs | verifiziert | Klasse 1: `Object.defineProperty` für `__proto__`-Sicherheit entfällt (Rust-Maps sind reine Daten); Reihenfolge und alle Ablehnungsregeln 1:1 |
| — (neu) | — | src/cbor/value.rs | verifiziert | Klasse 1: expliziter Wertetyp für JS-`unknown`; `Number` ist wie in JS ein f64. Enthält den Port von `isProtocolValue` (`to_json_value`) |
| src/framing.ts | 165 | src/framing.rs | verifiziert | Klasse 1: `FrameError`-Enum mit Varianten `Frame` und `Range` (TS `RangeError`); `FrameDecoder::new` gibt `Result` zurück statt zu werfen; 64-KiB-Blocklogik erhalten |
| src/codec.ts | 172 | src/codec.rs | verifiziert | Klasse 1: `isProtocolValue` ist in `CborValue::to_json_value` implementiert (Zyklen/`undefined`/Nicht-Plain-Objekte sind nicht darstellbar, Byte-Strings werden abgelehnt). `encodeProtocolMessage` validiert wie in TS vor dem Kodieren, indem es die Nachricht serialisiert und durch `parse_*` schickt. `isSupportedProtocolVersion` nimmt `u64` (die TS-`Number.isInteger`-Prüfung ist damit implizit) |
| src/schemas.ts | 450 | src/schemas.rs | verifiziert | Klasse 3 (Tech-Substitution TypeBox → serde): `additionalProperties:false` → `deny_unknown_fields`, `Type.Union` → `#[serde(untagged)]` in TS-Reihenfolge, `Type.Literal` → generierte Tag-Typen (`literal_tag!`), `minLength`/`minimum`/`minItems` → `deserialize_with`-Prüfungen, `Type.Optional` lehnt explizites `null` ab (wie TypeBox), `JsonValue` → `serde_json::Value`. `ResultForCommand<T>` ist eine reine TS-Typebene ohne Laufzeitwirkung und entfällt; `Command::name()`/`CommandResult::command()` liefern den Kommandonamen zur Laufzeit |
| test/cbor/cbor.test.ts | 175 | tests/cbor.rs | Tests portiert | Entfallene Fälle (nicht darstellbar, im Test dokumentiert): `undefined`-Properties, Array-Löcher, bigint/symbol/function/Date/Map, Symbol-Keys, verlustbehaftete Strings, Zyklen |
| test/framing.test.ts | 117 | tests/framing.rs | Tests portiert | `maxFrameLength` −1/1.5/NaN nicht darstellbar (u64); der Fall > MAX_UINT32 ist portiert |
| test/protocol.test.ts | 424 | tests/protocol.rs | Tests portiert | „rejects cyclic protocol values" nicht darstellbar. „validates messages before encoding" nutzt statt `version: 1.5` (nicht darstellbar) eine leere ID (`minLength: 1`) als äquivalente Constraint-Verletzung |

Testergebnis: 44 Tests (7 cbor, 12 framing, 25 protocol), alle grün.

## Ausschlüsse

| TS-Datei/Verzeichnis | Begründung (Master-Plan / Faktenbericht) |
|---|---|
| dist/ | Build-Artefakt |
| tsconfig*.json, vitest.config.ts, package.json | Distributionsmechanik (Master-Plan, Abweichungsklasse 4) |
