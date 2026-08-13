# Interface-Requests (workstream-übergreifender Kanal)

Einziger Kommunikationskanal zwischen den drei Workstreams. **Append-only**: neue
Einträge unten in der eigenen Sektion anhängen, fremde Einträge nie löschen oder
umschreiben. Der Owner der betroffenen Crate setzt um und hakt ab.

Ownership: A = `crates/notagent-tui/`, B = `crates/notagent-{telemetry,ai,agent}/`,
C = alles übrige (Root-Dateien, Protokoll-Crates, `crates/notagent/`).

Eintragsformat:

```
### <ID> <kurzer Titel>
- **Von / An**: <Workstream> → <Workstream>
- **Datum**: YYYY-MM-DD
- **Betrifft**: <Crate/Datei/Typ>
- **Beleg**: <TS-Quelle mit Zeilen, die das Verhalten festlegt>
- **Wunsch**: <konkret, mit Signaturvorschlag>
- **Status**: offen | umgesetzt (<commit>) | abgelehnt (<Begründung>)
```

IDs: `A-1`, `B-1`, `C-1`, … fortlaufend je Absender.

## Sektion A (Workstream A — TUI)

_(noch leer)_

## Sektion B (Workstream B — AI + Agent)

_(noch leer)_

## Sektion C (Workstream C — App)

_(noch leer)_
