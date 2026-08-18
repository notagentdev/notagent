# TUI-Zeilen teilen statt kopieren

## Objective

Die Kosten eines Repaints sollen nicht mehr linear mit der Länge des Transkripts
in Bytes wachsen, sondern nur noch mit der Zeilenzahl — und dort auf einen
Zeigervergleich fallen.

Heute kostet jeder Frame bei einem Transkript aus N Zeilen drei Allokationen und
einen vollständigen Bytevergleich pro Zeile, unabhängig davon, ob sich etwas
geändert hat. Das trifft jeden Spinner-Tick und jeden Streaming-Chunk und ist die
Ursache der spürbaren Trägheit in langen Sitzungen.

Die drei Allokationen entstehen an diesen Stellen:

- `crates/notagent-tui/src/components/text.rs:58` gibt bei einem Cache-**Treffer**
  eine Kopie der zwischengespeicherten Zeilenliste zurück, samt Inhaltskopie je
  Zeile.
- `crates/notagent-tui/src/tui.rs:1641` (`apply_line_resets`) schreibt jede Zeile
  bedingungslos neu; darin steckt einmal `normalize_terminal_output` und einmal
  das Anhängen des Segment-Resets.
- `crates/notagent-tui/src/utils.rs:505` legt im Normalfall — kein Tabulator,
  keine Thai-/Lao-Zeichen — trotzdem eine vollständige Kopie des Textes an.

Der Bytevergleich sitzt in der Diff-Schleife
`crates/notagent-tui/src/tui_main_screen.rs:498-508`, die für jede Zeile
`old_line != new_line` auswertet.

Zielzustand, wie er in `../notagent-main-rust` bereits umgesetzt ist:

- Der Zeilentyp ist geteilt statt besessen
  (`../notagent-main-rust/crates/notagent_tui/src/component.rs:23`), ein
  Cache-Treffer wird zur Refcount-Erhöhung.
- Der Diff entscheidet unveränderte Zeilen über Zeigeridentität
  (`../notagent-main-rust/crates/notagent_tui/src/tui.rs:914`), der Textvergleich
  bleibt als Rückfall erhalten.
- Der Reset-Durchlauf überspringt Zeilen, die ihn bereits tragen
  (`../notagent-main-rust/crates/notagent_tui/src/tui.rs:773`); die
  Markdown-Komponente stellt ihn im Cache voran
  (`../notagent-main-rust/crates/notagent_tui/src/markdown.rs:351`).

Nicht Ziel dieses Vorhabens: der Baumdurchlauf selbst. `do_render`
(`crates/notagent-tui/src/tui_main_screen.rs:425`) rendert weiterhin den ganzen
Komponentenbaum, weil `chat_container` fertige Blöcke behält
(`crates/notagent/src/modes/interactive/interactive_mode.rs:5957`). Dieser Anteil
ist O(Komponenten) statt O(Bytes) und wird erst nach der Messung bewertet.

## Umfang und Annahmen

Betroffen sind **78** Implementierungen des `Component`-Traits: 20 in
`crates/notagent-tui/src` und 58 in `crates/notagent/src`. In den Dateien, die
eine solche Implementierung enthalten, kommt `Vec<String>` **241**-mal vor;
insgesamt enthalten die beiden Crates 703 Vorkommen, von denen der Rest nichts
mit Zeilen zu tun hat und unangetastet bleibt.

Annahmen, die dieser Plan trifft:

- Der geteilte Zeilentyp wird `std::sync::Arc<str>`, nicht `Rc<str>`. Begründung
  der Referenz: gerenderte Zeilen überqueren Threadgrenzen, weil animierende
  Komponenten auf eigenen Threads laufen. Unser `Loader`
  (`crates/notagent-tui/src/components/loader.rs:157`) und `CancellableLoader`
  (`crates/notagent-tui/src/components/cancellable_loader.rs:56`) sind vor
  Schritt 5 daraufhin zu prüfen; ergibt die Prüfung, dass keine Zeile den Thread
  wechselt, bleibt `Arc` trotzdem die Wahl, weil der Unterschied im Nanosekunden-
  bereich liegt und die Referenz damit vergleichbar bleibt.
- Der Umbau ist eine bewusste Abweichung von der 1:1-Spiegelung der
  TS-Vorlage: In TypeScript sind Strings unveränderlich und geteilt, dort kostet
  die Rückgabe des Caches nichts. Die Abweichung ist in `CONVENTIONS.md` und in
  den Dateiköpfen zu vermerken.
- Die Schritte 1 bis 3 stehen für sich. Werden 4 bis 7 nie ausgeführt, bleiben
  sie ein Gewinn und richten keinen Schaden an.

## Implementation Plan

- [ ] 1. **`normalize_terminal_output` auf eine ausleihende Rückgabe umstellen.**
  Die Funktion in `crates/notagent-tui/src/utils.rs:493` gibt heute immer einen
  neuen `String` zurück, obwohl der Normalfall — Text ohne Tabulator und ohne die
  beiden behandelten Thai-/Lao-Zeichen — inhaltlich unverändert bleibt. Die
  Rückgabe soll auf einen ausleihenden Typ wechseln, der im unveränderten Fall
  den Eingabe-Slice durchreicht und nur bei tatsächlicher Ersetzung oder
  Tabulator-Expansion eine neue Zeichenkette anlegt. Damit entfällt eine der drei
  Allokationen pro Zeile pro Frame, ohne dass sich ein Typ an einer
  Schnittstelle ändert. Aufrufstellen: `crates/notagent-tui/src/tui.rs:1644`
  sowie alle weiteren, die vorab über eine Suche nach dem Funktionsnamen zu
  erheben und einzeln anzupassen sind. Das Verhalten bleibt bitgleich; die
  vorhandenen Zusicherungen in `crates/notagent-tui/tests/utils_oracle.rs`
  bleiben unverändert gültig und dienen als Nachweis.

- [ ] 2. **`apply_line_resets` idempotent machen.**
  `crates/notagent-tui/src/tui.rs:1641` schreibt jede Zeile neu. Künftig soll die
  Funktion eine Zeile unangetastet lassen, die den Segment-Reset bereits am Ende
  trägt, und weiterhin Bildzeilen aussparen, wie es die bestehende Prüfung über
  `is_image_line` (`crates/notagent-tui/src/terminal_image.rs:206`) tut. Die
  Referenz prüft zusätzlich auf die Kitty-Einleitungssequenz
  (`../notagent-main-rust/crates/notagent_tui/src/tui.rs:780`); ob unsere
  `is_image_line`-Prüfung dieselbe Menge abdeckt, ist beim Umbau zu belegen und
  gegebenenfalls anzugleichen. Betroffen sind alle drei Aufrufstellen:
  `crates/notagent-tui/src/tui_main_screen.rs:435`,
  `crates/notagent-tui/src/tui_alt_screen.rs:1865` und
  `crates/notagent-tui/src/tui_alt_screen.rs:2005`. Der Schritt ist die
  Voraussetzung dafür, dass ein späteres Voranstellen im Cache wirkt, und
  zugleich die Sicherheitsgarantie des ganzen Vorhabens: Eine Komponente, die den
  Reset nicht voranstellt, bleibt korrekt und ist nur langsamer.

- [ ] 3. **Den Repaint-Harness aus der Referenz übernehmen und Ausgangswerte
  festhalten.** `../notagent-main-rust/crates/notagent_tui/tests/render_cost.rs`
  (46 Zeilen) misst den Repaint eines unveränderten Transkripts bei 50, 200 und
  800 Blöcken und gibt Mikrosekunden je Frame und je Zeile aus, ohne etwas
  zuzusichern. Er ist als `#[ignore]`-Zeitmessung angelegt und wird ausdrücklich
  aufgerufen, damit er nicht in `scripts/check.sh` mitläuft. Er nutzt
  ausschließlich `Container`, `Text` und `Component::render` und ist deshalb
  gegen unsere API mit minimaler Anpassung lauffähig. Wichtig für die spätere
  Auswertung: Der Dateikopf der Referenz hält fest, dass bewusst kein Zeilentyp
  benannt wird, damit derselbe Harness gegen die besitzende und die geteilte
  Fassung kompiliert. Diese Eigenschaft ist bei der Übernahme zu erhalten. Die
  Messwerte nach den Schritten 1 und 2 sind als Ausgangslage im Plan oder in
  einem Kommentar der Datei zu vermerken.

- [ ] 4. **Eine Zusicherung für die Reset-Invariante schaffen.** Heute existiert
  im ganzen Baum kein Test, der prüft, dass gerenderte Zeilen den Segment-Reset
  tragen — eine Suche danach in `crates/notagent-tui/src` und
  `crates/notagent-tui/tests` bleibt ergebnislos. Solange
  `apply_line_resets` bedingungslos anhängt, kann nichts schiefgehen; sobald
  Schritt 7 die Verantwortung verschiebt, wird es die zentrale Fehlerquelle. Es
  ist deshalb ein Test anzulegen, der einen realistischen Komponentenbaum
  rendert, den Reset-Durchlauf anwendet und für jede Zeile belegt, dass sie
  entweder auf dem Reset endet oder eine Bildzeile ist. Die Referenz sichert
  dasselbe an der Komponente ab
  (`../notagent-main-rust/crates/notagent_tui/src/markdown.rs:1162`); beide
  Ebenen sind sinnvoll, die Baum-Ebene ist die wichtigere.

- [ ] 5. **Den geteilten Zeilentyp einführen und den Trait umstellen.** Ein
  Alias für die geteilte Zeile ist in `crates/notagent-tui/src/tui.rs` neben der
  `Component`-Definition anzulegen, mit einem Dateikopf-Kommentar, der die
  Entscheidung trägt: warum geteilt statt geliehen (ein Container führt seine
  Kinder zu einer flachen Liste zusammen und kann keine Referenz auf jedes Kind
  halten), warum atomar statt nicht-atomar, und dass der zweite Gewinn im Diff
  liegt. Die Signatur von `Component::render` wechselt auf eine Liste dieses
  Typs. Danach sind alle **78** Implementierungen anzupassen — 20 in
  `crates/notagent-tui/src`, 58 in `crates/notagent/src`, vollständige Liste über
  eine Suche nach `impl Component for` zu erheben. Der Compiler führt durch
  diesen Schritt; es sind keine Verhaltensänderungen beabsichtigt. Komponenten,
  die ihre Zeilen intern als besitzende Zeichenketten aufbauen, wandeln erst bei
  der Rückgabe um, wie es die Referenz tut.

- [ ] 6. **Die Verbraucher und die zeilenverändernden Stellen anpassen.** Der
  Typwechsel erreicht sechs Stellen, die Zeilen nach dem Rendern anfassen; jede
  braucht eine eigene Entscheidung:
  (a) `crates/notagent-tui/src/tui.rs:1652` (`extract_cursor_position`) schneidet
  den Cursor-Marker per Substring heraus und schreibt die Zeile zurück — betrifft
  eine Zeile pro Frame, der Neubau ist unkritisch.
  (b) `crates/notagent-tui/src/tui.rs:1550` (`composite_overlays`) und
  `crates/notagent-tui/src/tui.rs:248` (`composite_tui_line`) bauen Zeilen aus
  Teilstücken zusammen; beide erzeugen ohnehin neue Zeichenketten.
  (c) `crates/notagent-tui/src/components/h_stack.rs:109` und
  `crates/notagent-tui/src/layout.rs:578` rufen dieselbe Compositing-Funktion auf.
  (d) `crates/notagent-tui/src/layout.rs:86` hält einen **zweiten** Render-Cache,
  dessen Zugriff in `crates/notagent-tui/src/layout.rs:108` sowohl beim Treffer
  als auch beim Eintragen klont — hier verdoppelt sich heute die Kopierarbeit für
  alle Komponenten, die über die Layout-Engine laufen. Diese Stelle hat in der
  Referenz keine Entsprechung und ist eigenständig zu entscheiden.
  (e) Die Zustandsfelder `previous_lines`
  (`crates/notagent-tui/src/tui_main_screen.rs:65` und `:83`) und
  `previous_screen` (`crates/notagent-tui/src/tui_alt_screen.rs:118`) wechseln
  den Elementtyp mit.
  (f) Die Kitty-Behandlung liest Zeileninhalte
  (`crates/notagent-tui/src/tui_main_screen.rs:49`, `:285`, `:309`); sie liest
  nur und braucht keine Änderung außer der Signatur.

- [ ] 7. **Den Diff auf Zeigeridentität umstellen.** Die Schleife in
  `crates/notagent-tui/src/tui_main_screen.rs:498-508` vergleicht heute jede
  Zeile über ihren vollen Inhalt. Künftig soll zuerst die Zeigeridentität
  geprüft werden und der Inhaltsvergleich nur noch für Zeilen laufen, die diese
  Prüfung nicht bestehen — genau in der Form, die die Referenz in
  `../notagent-main-rust/crates/notagent_tui/src/tui.rs:914` verwendet. Der
  Inhaltsvergleich **bleibt** erhalten: Die Zeigergleichheit ist eine Abkürzung
  über einem korrekten Vergleich, kein Ersatz. Geht die Zeigeridentität verloren,
  wird der Frame langsamer, nicht falsch. Der Alt-Screen-Pfad vergleicht an
  `crates/notagent-tui/src/tui_alt_screen.rs:1461` auf gleiche Weise und ist
  mitzuziehen. Dies ist der Schritt, der den eigentlichen Gewinn bringt.

- [ ] 8. **Den Reset in den Cache der Markdown-Komponente voranstellen.** In
  `crates/notagent-tui/src/components/markdown.rs:891` ist der Reset beim Füllen
  des Caches an jede Zeile anzuhängen, sodass der Durchlauf aus Schritt 2 sie
  überspringt. Die Markdown-Komponente ist gewählt, weil sie den Transkriptrumpf
  erzeugt und damit die große Masse der Zeilen; die Referenz hat genau diese eine
  Komponente umgestellt und alle übrigen unangetastet gelassen. Ein Test an der
  Komponente sichert die Eigenschaft ab. Weitere Komponenten sind erst dann zu
  behandeln, wenn die Messung aus Schritt 3 zeigt, dass sie ins Gewicht fallen.

- [ ] 9. **Die Messung wiederholen und das Ergebnis festhalten.** Der Harness aus
  Schritt 3 ist erneut zu starten und den Ausgangswerten gegenüberzustellen,
  getrennt nach 50, 200 und 800 Blöcken. Das Ergebnis gehört als Kommentar in die
  Harness-Datei, damit die nächste Änderung eine Bezugsgröße hat. Fällt der
  Gewinn deutlich geringer aus als erwartet, ist der Baumdurchlauf der nächste
  Verdächtige und in einem eigenen Vorhaben zu behandeln.

- [ ] 10. **Abweichung und Konventionen nachtragen.** In `CONVENTIONS.md` ist zu
  vermerken, dass der Zeilentyp des TUI bewusst von der TS-Vorlage abweicht, mit
  der Begründung aus dem Objective. Die Dateiköpfe der geänderten Dateien folgen
  der bestehenden Klassifizierung von Abweichungen. Die Commit-Betreffzeilen
  beginnen mit `fix:` beziehungsweise `feature:`; die Schritte 1, 2 und 7 sind
  `fix:`, die Schritte 3, 4 und 5 `feature:`. Eine Ledger-Pflege entfällt, da
  `PARITY.md` stillgelegt ist.

## Verification Criteria

- `scripts/check.sh` läuft vollständig grün: `cargo fmt --check`, `cargo clippy
  --workspace --all-targets -- -D warnings` und `cargo test --workspace`. Die
  Suite umfasst 43 Testdateien allein in `crates/notagent-tui/tests`, davon 39,
  die `render` aufrufen.
- Nach Schritt 2 belegt ein Test, dass ein zweiter Aufruf des Reset-Durchlaufs
  auf dieselben Zeilen keine Änderung mehr bewirkt.
- Nach Schritt 4 existiert eine Zusicherung, die für jede Zeile eines gerenderten
  Baums entweder den abschließenden Reset oder eine Bildzeile nachweist; sie ist
  vor dem Typwechsel grün.
- Nach Schritt 5 kompiliert der Workspace ohne Warnungen, und keine der 78
  Trait-Implementierungen gibt mehr eine besitzende Zeilenliste zurück.
- Nach Schritt 7 belegt ein Test, dass zwei aufeinanderfolgende Repaints ohne
  Inhaltsänderung keine geänderte Zeile melden, und dass eine geänderte Zeile
  weiterhin erkannt wird, wenn ihr Inhalt gleich, ihre Identität aber neu ist.
- Die visuelle Prüfung im laufenden Programm über `notagent-test.sh` zeigt
  unveränderte Darstellung für: Cursorposition im Editor, ein geöffnetes Overlay,
  ein Auswahldialog, eine Bildausgabe im Kitty-Protokoll, ein langes
  Markdown-Transkript und das Verhalten beim Verkleinern des Fensters.
- Der Harness aus Schritt 3 liefert bei 800 Blöcken eine messbar niedrigere Zeit
  je Frame als in der Ausgangsmessung; der konkrete Faktor wird nicht vorab
  festgelegt, sondern dokumentiert.

## Potential Risks and Mitigations

1. **Cursorposition, Overlays und Bildausgabe brechen sichtbar, ohne dass ein
   Test anschlägt.** Diese drei Pfade erzeugen Terminal-Escape-Sequenzen; ein
   Fehler äußert sich als falsch platzierter Cursor, verrutschtes Overlay oder
   zerstörte Grafik und wird von Zusicherungen über Zeichenketten nicht
   zuverlässig gefangen.
   Mitigation: Schritt 6 behandelt jede der sechs Stellen einzeln und benannt
   statt als Sammeländerung. Die visuelle Prüfliste in den
   Verifikationskriterien ist verbindlich und wird im laufenden Programm
   abgearbeitet, nicht nur in der Suite.

2. **Der Kitty-Ausschluss wird beim Umbau von `apply_line_resets` enger oder
   weiter als heute.** Ein Reset innerhalb einer Grafiksequenz zerstört die
   Ausgabe; ein fehlender Reset auf einer normalen Zeile lässt Farbe ausbluten.
   Mitigation: Schritt 2 verlangt ausdrücklich den Nachweis, dass die neue
   Bedingung dieselbe Zeilenmenge ausspart wie die heutige `is_image_line`-
   Prüfung, gemessen an den vorhandenen Tests in
   `crates/notagent-tui/tests/terminal_image.rs`.

3. **Der Nutzen bleibt aus, weil eine übersehene Stelle weiterhin je Zeile
   allokiert.** Der zweite Render-Cache in `crates/notagent-tui/src/layout.rs:108`
   ist der wahrscheinlichste Kandidat, weil er sowohl beim Treffer als auch beim
   Eintragen klont und in der Referenz kein Gegenstück hat.
   Mitigation: Schritt 6 (d) führt ihn als eigene Entscheidung. Schritt 9 misst
   nach; bleibt der Gewinn aus, ist die Messung der Wegweiser statt einer
   Vermutung.

4. **Der Typwechsel über 78 Implementierungen wird zu einem einzigen,
   unüberprüfbaren Commit.**
   Mitigation: Die Schritte 1 bis 4 sind eigenständig und werden einzeln
   eingebracht. Schritt 5 ist compilergeführt und kann nach Crate getrennt
   erfolgen — erst `crates/notagent-tui`, dann `crates/notagent` —, weil die
   Trait-Definition im ersten liegt.

5. **Nebenläufigkeit: eine animierende Komponente mutiert eine Zeile, während der
   UI-Thread rendert.** Die Referenz nennt genau diesen Fall als Grund für den
   atomaren Refcount.
   Mitigation: Vor Schritt 5 sind `crates/notagent-tui/src/components/loader.rs`
   und `crates/notagent-tui/src/components/cancellable_loader.rs` daraufhin zu
   prüfen und das Ergebnis im Kommentar des Zeilentyps festzuhalten.

6. **Die Abweichung von der TS-Vorlage wird später als Portierungsfehler
   gelesen.**
   Mitigation: Schritt 10 trägt sie in `CONVENTIONS.md` und in den Dateiköpfen
   nach, mit der Begründung, dass geteilte unveränderliche Zeichenketten in
   TypeScript kostenlos sind und in Rust nicht.

## Alternative Approaches

1. **Nur die Schritte 1 und 2 umsetzen und den Typwechsel lassen.** Beseitigt
   zwei der drei Allokationen pro Zeile, lässt den Bytevergleich im Diff
   unangetastet und ändert keine Signatur. Deutlich geringeres Risiko, deutlich
   geringerer Gewinn — der Diff ist der Posten, der über die volle Zeilenlänge
   geht. Sinnvoll als Rückfallposition, wenn Schritt 5 sich als zu weitreichend
   erweist.

2. **Fertige Blöcke aus dem Komponentenbaum entfernen, statt Zeilen zu teilen.**
   Wenn abgeschlossene Transkriptblöcke in den Scrollback geschrieben und aus
   `chat_container` gelöscht würden, sänke die Frame-Arbeit auf das sichtbare
   Fenster — der Ansatz, den ein zellbasierter Renderer von Haus aus verfolgt.
   Das ist die wirksamere Lösung und zugleich die weit größere: Sie berührt
   Bildlauf, Neuzeichnen bei Größenänderung, die Suche im Alt-Screen und das
   Wiederherstellen einer Sitzung. Kein Widerspruch zu diesem Plan, sondern ein
   mögliches Nachfolgevorhaben.

3. **Einen nicht-atomaren geteilten Zeilentyp verwenden.** Spart die atomare
   Zählerarbeit, verlangt aber den Nachweis, dass keine gerenderte Zeile je einen
   Thread wechselt. Die Referenz hat sich bewusst dagegen entschieden und den
   Grund dokumentiert; ohne eigene Messung, die einen Unterschied belegt, ist die
   Abweichung von der Referenz nicht zu rechtfertigen.

4. **Die Zeilenliste durch eine Struktur mit vorberechneten Merkmalen ersetzen**
   — etwa sichtbare Breite und ein Bildzeilen-Kennzeichen —, um die wiederholten
   Inhaltsprüfungen in `apply_line_resets` und der Kitty-Behandlung zu sparen.
   Löst ein reales Problem, das auch die Referenz noch hat: deren Reset-Durchlauf
   durchsucht jede Zeile nach der Grafiksequenz. Erhöht jedoch die Reichweite des
   Umbaus erheblich und sollte erst nach der Messung aus Schritt 9 bewertet
   werden.
