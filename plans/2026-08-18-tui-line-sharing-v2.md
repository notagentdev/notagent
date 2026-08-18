# TUI-Zeilen teilen statt kopieren

## Objective

Die Kosten eines Repaints sollen nicht mehr mit der Länge des Transkripts in
Bytes wachsen, sondern nur noch mit der Zeilenzahl — und für unveränderte Zeilen
auf einen Zeigervergleich fallen. Die Zeilenliste selbst wird weiterhin pro
Komponente pro Frame aufgebaut; das ist eine Allokation über Zeiger und bleibt
stehen. Wer „auf einen Zeigervergleich" liest, soll es auf den Vergleich beziehen,
nicht auf den gesamten Frame.

Heute kostet jeder Frame bei einem Transkript aus N Zeilen drei Allokationen und
einen vollständigen Bytevergleich pro Zeile, unabhängig davon, ob sich etwas
geändert hat. Das trifft jeden Spinner-Tick und jeden Streaming-Chunk und ist die
Ursache der spürbaren Trägheit in langen Sitzungen.

Die drei Allokationen entstehen an diesen Stellen:

- `crates/notagent-tui/src/components/text.rs:63` gibt bei einem Cache-**Treffer**
  eine Tiefkopie der zwischengespeicherten Zeilenliste zurück, samt Inhaltskopie
  je Zeile.
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

## Zwei Vorlagen, und wo sie auseinandergehen

Für dieses Vorhaben gibt es zwei Bezugspunkte, und sie widersprechen sich an
einer Stelle. `../pi-main` ist die TypeScript-Vorlage, die dieser Port spiegelt;
`../notagent-main-rust` ist die Rust-Referenz, die den Umbau bereits gemacht hat.
Wo beide dasselbe sagen, ist die Sache klar. Wo nicht, gilt die Vorlage, sofern
die Abweichung der Referenz nicht als bewusste Entscheidung belegt ist.

- **Der Malpfad normalisiert — in der Vorlage wie bei uns.**
  `../pi-main/packages/tui/src/tui.ts:1158` ruft in `applyLineResets` für jede
  Nicht-Bildzeile `normalizeTerminalOutput` auf und hängt den Reset an; unser
  `crates/notagent-tui/src/tui.rs:1644` tut dasselbe. Die Rust-Referenz tut es
  nicht (`../notagent-main-rust/crates/notagent_tui/src/tui.rs:773`), und sie hat
  die Behandlung auch nirgends sonst: die Thai-/Lao-Codepunkte kommen in ihrer
  gesamten TUI-Kiste nicht vor. Sie hat die Funktion also nicht verschoben,
  sondern das Verhalten verloren. Der Aufruf bleibt daher stehen; was wegfällt,
  ist nur seine Allokation — siehe den nächsten Punkt.
- **In der Vorlage kostet der Aufruf nichts.**
  `../pi-main/packages/tui/src/utils.ts:386` gibt die Eingabe unverändert zurück,
  sobald weder Thai-/Lao-Zeichen noch ein Tabulator vorkommen — in JavaScript ist
  das dieselbe Zeichenkette, keine Kopie. Unser
  `crates/notagent-tui/src/utils.rs:505` legt an derselben Stelle eine
  vollständige Kopie an. Diese Allokation ist ein Portierungsartefakt, kein
  Verhalten der Vorlage, und Schritt 1 beseitigt genau sie.
- **Die Bildzeilen-Prüfung ist bei uns die der Vorlage.**
  `../pi-main/packages/tui/src/tui.ts:1157` prüft über `isImageLine`, wir über
  `is_image_line` (`crates/notagent-tui/src/terminal_image.rs:206`). Die
  Rust-Referenz prüft stattdessen direkt auf die Kitty-Einleitungssequenz. Hier
  ist also die Referenz die Abweichung; unsere Prüfung bleibt, und Schritt 2 hat
  lediglich zu belegen, dass die neue Bedingung dieselbe Menge ausspart wie die
  heutige.
- **Wir haben drei Aufrufstellen des Reset-Durchlaufs, die Referenz eine.** Die
  Vorlage hat ebenfalls drei (`../pi-main/packages/tui/src/tui-main-screen.ts:207`
  und `../pi-main/packages/tui/src/tui-alt-screen.ts:309`, `:1240`), unsere
  liegen in `crates/notagent-tui/src/tui_main_screen.rs:435`,
  `crates/notagent-tui/src/tui_alt_screen.rs:1865` und
  `crates/notagent-tui/src/tui_alt_screen.rs:2005`. Jede Zusicherung über den
  Reset gilt bei uns also dreimal, und Schritt 8 wirkt nur dort, wo der Durchlauf
  auch läuft — das ist gegenüber der Referenz zusätzliche Fläche.

Der zweite Render-Cache in `crates/notagent-tui/src/layout.rs:108` hat in der
Rust-Referenz keine Entsprechung; er wird in Schritt 6 als eigene Entscheidung
geführt.

## Umfang und Annahmen

Betroffen sind **78** Implementierungen des `Component`-Traits: 20 in
`crates/notagent-tui/src` und 58 in `crates/notagent/src`.

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
- Die Schritte 1 bis 4 stehen für sich. Werden 5 bis 8 nie ausgeführt, bleiben
  sie ein Gewinn und richten keinen Schaden an.
- Der Gewinn aus Schritt 7 ist gedeckelt durch den Anteil der Zeilen, die aus
  cachenden Komponenten stammen. Wie hoch der ist, wird in Schritt 3 gemessen
  und nicht geschätzt.

## Implementation Plan

- [x] 1. **`normalize_terminal_output` auf eine ausleihende Rückgabe umstellen,
  damit sie kostet, was sie in der Vorlage kostet.** Die Funktion in
  `crates/notagent-tui/src/utils.rs:493` gibt heute immer einen neuen `String`
  zurück, obwohl der Normalfall — kein Tabulator, keine Thai-/Lao-Zeichen —
  inhaltlich unverändert bleibt. Die Vorlage gibt an derselben Stelle die
  Eingabe selbst zurück (`../pi-main/packages/tui/src/utils.ts:386`) und zahlt
  dafür nichts; unsere Kopie ist ein Portierungsartefakt. Die Rückgabe soll auf
  einen ausleihenden Typ wechseln, der im unveränderten Fall den Eingabe-Slice
  durchreicht und nur bei tatsächlicher Ersetzung oder Tabulator-Expansion eine
  neue Zeichenkette anlegt. Damit entfällt eine der drei Allokationen pro Zeile
  pro Frame, ohne dass sich ein Typ an einer Schnittstelle ändert und ohne
  Verhaltensänderung. Der Aufruf bleibt ausdrücklich im Malpfad: Die
  Rust-Referenz hat ihn dort nicht, hat die Thai-/Lao-Behandlung aber auch
  nirgends sonst, also das Verhalten verloren statt verschoben — dem ist nicht zu
  folgen. Aufrufstellen sind vorab über eine Suche nach dem Funktionsnamen
  vollständig zu erheben und einzeln anzupassen. Das Verhalten bleibt bitgleich;
  die vorhandenen Zusicherungen in `crates/notagent-tui/tests/utils_oracle.rs`
  bleiben gültig und dienen als Nachweis.

- [x] 2. **`apply_line_resets` idempotent machen, an allen drei Aufrufstellen.**
  `crates/notagent-tui/src/tui.rs:1641` schreibt jede Zeile neu. Künftig soll die
  Funktion eine Zeile unangetastet lassen, die den Segment-Reset bereits am Ende
  trägt, und weiterhin Bildzeilen aussparen. Die Prüfung bleibt
  `is_image_line` (`crates/notagent-tui/src/terminal_image.rs:206`), weil die
  Vorlage an dieser Stelle dasselbe tut
  (`../pi-main/packages/tui/src/tui.ts:1157`); die direkte Sequenzprüfung der
  Rust-Referenz ist deren Abweichung und nicht zu übernehmen. Nachzuweisen ist
  nur, dass die neue Bedingung dieselbe Zeilenmenge ausspart wie die heutige,
  gemessen an `crates/notagent-tui/tests/terminal_image.rs`. Anders als in der
  Rust-Referenz betrifft das drei Stellen —
  `crates/notagent-tui/src/tui_main_screen.rs:435`,
  `crates/notagent-tui/src/tui_alt_screen.rs:1865` und
  `crates/notagent-tui/src/tui_alt_screen.rs:2005` —, die alle dieselbe Invariante
  tragen müssen. Der Schritt ist die Voraussetzung dafür, dass ein späteres
  Voranstellen im Cache wirkt, und zugleich die Sicherheitsgarantie des ganzen
  Vorhabens: Eine Komponente, die den Reset nicht voranstellt, bleibt korrekt und
  ist nur langsamer.

- [x] 3. **Den Repaint-Harness aus der Referenz übernehmen, Ausgangswerte
  festhalten und den Cache-Anteil erheben.**
  `../notagent-main-rust/crates/notagent_tui/tests/render_cost.rs` (46 Zeilen)
  misst den Repaint eines unveränderten Transkripts bei 50, 200 und 800 Blöcken
  und gibt Mikrosekunden je Frame und je Zeile aus, ohne etwas zuzusichern. Er ist
  als `#[ignore]`-Zeitmessung angelegt und wird ausdrücklich aufgerufen, damit er
  nicht in `scripts/check.sh` mitläuft. Er nutzt ausschließlich `Container`,
  `Text` und `Component::render` und ist gegen unsere API mit minimaler Anpassung
  lauffähig. Bei der Übernahme ist zu erhalten, dass er bewusst keinen Zeilentyp
  benennt, damit dieselbe Datei gegen die besitzende und die geteilte Fassung
  kompiliert. Zusätzlich zur Referenz ist zu erheben, welcher Anteil der Zeilen
  eines realistischen Transkripts aus Komponenten mit Cache stammt — das ist die
  Obergrenze für den Gewinn aus Schritt 7 und die Zahl, an der Schritt 9 gemessen
  wird. Die Werte nach den Schritten 1 und 2 sind als Ausgangslage in einem
  Kommentar der Harness-Datei zu vermerken.

- [x] 4. **Eine Zusicherung für die Reset-Invariante schaffen.** Heute existiert
  im ganzen Baum kein Test, der prüft, dass gerenderte Zeilen den Segment-Reset
  tragen. Solange `apply_line_resets` bedingungslos anhängt, kann nichts
  schiefgehen; sobald Schritt 8 die Verantwortung verschiebt, wird es die zentrale
  Fehlerquelle. Es ist deshalb ein Test anzulegen, der einen realistischen
  Komponentenbaum rendert, den Reset-Durchlauf anwendet und für jede Zeile belegt,
  dass sie entweder auf dem Reset endet oder eine Bildzeile ist. Die Referenz
  sichert dasselbe an der Komponente ab
  (`../notagent-main-rust/crates/notagent_tui/src/markdown.rs:1162`); beide Ebenen
  sind sinnvoll, die Baum-Ebene ist die wichtigere. Weil der Durchlauf bei uns an
  drei Stellen sitzt, ist die Zusicherung so anzulegen, dass sie für alle drei
  gilt und nicht nur für den Hauptbildschirm.

- [x] 5. **Den geteilten Zeilentyp einführen und den Trait umstellen.** Ein
  Alias für die geteilte Zeile ist in `crates/notagent-tui/src/tui.rs` neben der
  `Component`-Definition anzulegen, mit einem Kommentar, der die Entscheidung
  trägt: warum geteilt statt geliehen (ein Container führt seine Kinder zu einer
  flachen Liste zusammen und kann keine Referenz auf jedes Kind halten), warum
  atomar statt nicht-atomar, und dass der zweite Gewinn im Diff liegt. Die
  Signatur von `Component::render` wechselt auf eine Liste dieses Typs. Danach
  sind alle **78** Implementierungen anzupassen — 20 in `crates/notagent-tui/src`,
  58 in `crates/notagent/src`, vollständige Liste über eine Suche nach
  `impl Component for` zu erheben. Der Compiler führt durch diesen Schritt; es
  sind keine Verhaltensänderungen beabsichtigt. Komponenten, die ihre Zeilen
  intern als besitzende Zeichenketten aufbauen, wandeln erst bei der Rückgabe um,
  wie es die Referenz tut.

- [x] 6. **Die Verbraucher und die zeilenverändernden Stellen anpassen.** Der
  Typwechsel erreicht sechs Stellen, die Zeilen nach dem Rendern anfassen; jede
  braucht eine eigene Entscheidung:
  (a) `crates/notagent-tui/src/tui.rs:1652` (`extract_cursor_position`) schneidet
  den Cursor-Marker per Substring heraus und schreibt die Zeile zurück — betrifft
  eine Zeile pro Frame, der Neubau ist unkritisch und entspricht dem, was die
  Referenz an derselben Stelle tut.
  (b) `crates/notagent-tui/src/tui.rs:1550` (`composite_overlays`) und
  `crates/notagent-tui/src/tui.rs:248` (`composite_tui_line`) bauen Zeilen aus
  Teilstücken zusammen; beide erzeugen ohnehin neue Zeichenketten.
  (c) `crates/notagent-tui/src/components/h_stack.rs:109` und
  `crates/notagent-tui/src/layout.rs:578` rufen dieselbe Compositing-Funktion auf.
  (d) `crates/notagent-tui/src/layout.rs:108` hält einen **zweiten** Render-Cache,
  dessen Zugriff in `crates/notagent-tui/src/layout.rs:116` beim Treffer klont —
  hier verdoppelt sich heute die Kopierarbeit für alle Komponenten, die über die
  Layout-Engine laufen. Diese Stelle hat in der Referenz keine Entsprechung und
  ist eigenständig zu entscheiden.
  (e) Die Zustandsfelder `previous_lines`
  (`crates/notagent-tui/src/tui_main_screen.rs:65` und `:83`) und
  `previous_screen` (`crates/notagent-tui/src/tui_alt_screen.rs:118`) wechseln
  den Elementtyp mit.
  (f) Die Kitty-Behandlung liest Zeileninhalte
  (`crates/notagent-tui/src/tui_main_screen.rs:49`, `:285`, `:309`); sie liest
  nur und braucht keine Änderung außer der Signatur.

- [x] 7. **Den Diff auf Zeigeridentität umstellen.** Die Schleife in
  `crates/notagent-tui/src/tui_main_screen.rs:498-508` vergleicht heute jede
  Zeile über ihren vollen Inhalt. Künftig soll zuerst die Zeigeridentität geprüft
  werden und der Inhaltsvergleich nur noch für Zeilen laufen, die diese Prüfung
  nicht bestehen — genau in der Form, die die Referenz in
  `../notagent-main-rust/crates/notagent_tui/src/tui.rs:914` verwendet. Der
  Inhaltsvergleich **bleibt** erhalten: Die Zeigergleichheit ist eine Abkürzung
  über einem korrekten Vergleich, kein Ersatz. Geht die Zeigeridentität verloren,
  wird der Frame langsamer, nicht falsch. Der Alt-Screen-Pfad vergleicht an
  `crates/notagent-tui/src/tui_alt_screen.rs:1461` auf gleiche Weise und ist
  mitzuziehen. Dies ist der Schritt, der den eigentlichen Gewinn bringt — soweit
  die Zeilen aus cachenden Komponenten kommen; für alle übrigen kostet er einen
  Zeigervergleich zusätzlich zum Inhaltsvergleich.

- [x] 8. **Den Reset in den Cache der Markdown-Komponente voranstellen.** In
  `crates/notagent-tui/src/components/markdown.rs:891` ist der Reset beim Füllen
  des Caches an jede Zeile anzuhängen, sodass der Durchlauf aus Schritt 2 sie
  überspringt. Die Markdown-Komponente ist gewählt, weil sie den Transkriptrumpf
  erzeugt und damit die große Masse der Zeilen; die Referenz hat genau diese eine
  Komponente umgestellt und alle übrigen unangetastet gelassen
  (`../notagent-main-rust/crates/notagent_tui/src/markdown.rs:351`). Ein Test an
  der Komponente sichert die Eigenschaft ab. Weitere Komponenten sind erst dann zu
  behandeln, wenn die Messung aus Schritt 3 zeigt, dass sie ins Gewicht fallen.

- [x] 9. **Nachmessen und vorab entscheiden, was bei welchem Ergebnis geschieht.**
  Der Harness aus Schritt 3 ist erneut zu starten und den Ausgangswerten
  gegenüberzustellen, getrennt nach 50, 200 und 800 Blöcken. Vor der Messung — und
  nicht danach — ist festzulegen, welches Ergebnis den Umbau trägt und welches
  ihn zurücknimmt: Bleibt die Zeit je Frame bei 800 Blöcken innerhalb der
  Messstreuung der Ausgangslage, sind die Schritte 5 bis 7 zurückzunehmen und die
  Schritte 1 bis 4 zu behalten, statt einen Typwechsel über 78 Implementierungen
  ohne belegten Nutzen stehen zu lassen. Der Grenzwert ist beim Festhalten der
  Ausgangswerte zu benennen. Das Ergebnis gehört als Kommentar in die
  Harness-Datei, damit die nächste Änderung eine Bezugsgröße hat. Fällt der Gewinn
  vorhanden, aber deutlich geringer aus als erwartet, ist der Anteil aus Schritt 3
  der erste Erklärungsversuch und der Baumdurchlauf der zweite.

- [x] 10. **Abweichung und Konventionen nachtragen.** In `CONVENTIONS.md` ist zu
  vermerken, dass der Zeilentyp des TUI bewusst von der TS-Vorlage abweicht, mit
  der Begründung aus dem Objective. Ebenfalls festzuhalten ist, an welchen zwei
  Stellen die Rust-Referenz von der TS-Vorlage abweicht — fehlende Normalisierung
  im Malpfad und direkte Sequenzprüfung statt Bildzeilen-Helfer — und dass wir
  ihr dort bewusst nicht folgen, damit die nächste Übernahme aus derselben
  Referenz nicht erneut darauf hereinfällt. Die Dateiköpfe der
  geänderten Dateien folgen der bestehenden Klassifizierung von Abweichungen. Die
  Commit-Betreffzeilen beginnen mit `fix:` beziehungsweise `feature:`; die
  Schritte 1, 2 und 7 sind `fix:`, die Schritte 3, 4 und 5 `feature:`. Eine
  Ledger-Pflege entfällt, da `PARITY.md` stillgelegt ist.

## Verification Criteria

- `scripts/check.sh` läuft vollständig grün: `cargo fmt --check`, `cargo clippy
  --workspace --all-targets -- -D warnings` und `cargo test --workspace`.
- Nach Schritt 1 legt `normalize_terminal_output` im unveränderten Fall nichts
  mehr an, und die Zusicherungen in `crates/notagent-tui/tests/utils_oracle.rs`
  sind unverändert grün — die Behandlung selbst bleibt im Malpfad, wie in der
  Vorlage.
- Nach Schritt 2 belegt ein Test, dass ein zweiter Aufruf des Reset-Durchlaufs
  auf dieselben Zeilen keine Änderung mehr bewirkt, und ein zweiter, dass die
  ausgesparte Zeilenmenge dieselbe ist wie vor dem Umbau.
- Nach Schritt 3 ist der Anteil der Zeilen aus cachenden Komponenten beziffert
  und in der Harness-Datei vermerkt.
- Nach Schritt 4 existiert eine Zusicherung, die für jede Zeile eines gerenderten
  Baums entweder den abschließenden Reset oder eine Bildzeile nachweist, und die
  für alle drei Aufrufstellen gilt; sie ist vor dem Typwechsel grün.
- Nach Schritt 5 kompiliert der Workspace ohne Warnungen, und keine der 78
  Trait-Implementierungen gibt mehr eine besitzende Zeilenliste zurück.
- Nach Schritt 7 belegt ein Test, dass zwei aufeinanderfolgende Repaints ohne
  Inhaltsänderung keine geänderte Zeile melden, und dass eine geänderte Zeile
  weiterhin erkannt wird, wenn ihr Inhalt gleich, ihre Identität aber neu ist.
- Die visuelle Prüfung im laufenden Programm über `notagent-test.sh` zeigt
  unveränderte Darstellung für: Cursorposition im Editor, ein geöffnetes Overlay,
  ein Auswahldialog, eine Bildausgabe im Kitty-Protokoll, ein langes
  Markdown-Transkript und das Verhalten beim Verkleinern des Fensters.
- Der Grenzwert aus Schritt 9 steht vor der Nachmessung fest, und das Ergebnis
  ist an ihm gemessen festgehalten.

## Potential Risks and Mitigations

1. **Der Zeigervergleich greift zu selten, und der Umbau kostet mehr als er
   bringt.** Zeigeridentität hilft nur, wo dieselbe geteilte Zeile über zwei
   Frames zurückkommt, also nur bei Komponenten mit Cache. Eine Komponente, die
   ihre Zeilen jeden Frame neu baut, liefert jedes Mal eine neue Zeile: Der
   Zeigervergleich schlägt fehl, der Inhaltsvergleich läuft trotzdem, und die
   Zeile kostet zusätzlich die Refcount-Arbeit.
   Mitigation: Schritt 3 beziffert den Anteil vor dem Umbau, Schritt 9 misst ihn
   gegen das Ergebnis, und der Grenzwert aus Schritt 9 nimmt den Umbau zurück,
   wenn er sich nicht trägt.

2. **Cursorposition, Overlays und Bildausgabe brechen sichtbar, ohne dass ein
   Test anschlägt.** Diese drei Pfade erzeugen Terminal-Escape-Sequenzen; ein
   Fehler äußert sich als falsch platzierter Cursor, verrutschtes Overlay oder
   zerstörte Grafik und wird von Zusicherungen über Zeichenketten nicht
   zuverlässig gefangen.
   Mitigation: Schritt 6 behandelt jede der sechs Stellen einzeln und benannt
   statt als Sammeländerung. Die visuelle Prüfliste in den
   Verifikationskriterien ist verbindlich und wird im laufenden Programm
   abgearbeitet, nicht nur in der Suite.

3. **Der Kitty-Ausschluss wird beim Umbau von `apply_line_resets` enger oder
   weiter als heute.** Ein Reset innerhalb einer Grafiksequenz zerstört die
   Ausgabe; ein fehlender Reset auf einer normalen Zeile lässt Farbe ausbluten.
   Die Gefahr liegt nicht im Helfer selbst — der entspricht der Vorlage —,
   sondern darin, dass die neue Abbruchbedingung die Menge nebenbei verschiebt.
   Mitigation: Schritt 2 verlangt den Nachweis der Deckungsgleichheit gegen
   `crates/notagent-tui/tests/terminal_image.rs`, bevor die Bedingung sich ändert.

   Ausdrücklich **nicht** zu übernehmen ist die direkte Sequenzprüfung der
   Rust-Referenz: Sie ist deren Abweichung von der Vorlage, und ein Wechsel
   darauf würde die ausgesparte Menge ohne Not verändern.

4. **Die Invariante wird nur an einer der drei Aufrufstellen hergestellt.**
   Anders als in der Referenz gibt es bei uns drei; eine übersehene lässt
   Markdown-Zeilen im Alt-Screen ohne Reset durch, sobald Schritt 8 wirkt.
   Mitigation: Schritt 2 und Schritt 4 nennen alle drei ausdrücklich, und die
   Zusicherung aus Schritt 4 gilt für alle drei.

5. **Der Nutzen bleibt aus, weil eine übersehene Stelle weiterhin je Zeile
   allokiert.** Der zweite Render-Cache in `crates/notagent-tui/src/layout.rs:108`
   ist der wahrscheinlichste Kandidat, weil er beim Treffer klont und in der
   Referenz kein Gegenstück hat.
   Mitigation: Schritt 6 (d) führt ihn als eigene Entscheidung. Schritt 9 misst
   nach; bleibt der Gewinn aus, ist die Messung der Wegweiser statt einer
   Vermutung.

6. **Der Typwechsel über 78 Implementierungen wird zu einem einzigen,
   unüberprüfbaren Commit.**
   Mitigation: Die Schritte 1 bis 4 sind eigenständig und werden einzeln
   eingebracht. Schritt 5 ist compilergeführt und kann nach Crate getrennt
   erfolgen — erst `crates/notagent-tui`, dann `crates/notagent` —, weil die
   Trait-Definition im ersten liegt.

7. **Nebenläufigkeit: eine animierende Komponente mutiert eine Zeile, während der
   UI-Thread rendert.** Die Referenz nennt genau diesen Fall als Grund für den
   atomaren Refcount.
   Mitigation: Vor Schritt 5 sind `crates/notagent-tui/src/components/loader.rs`
   und `crates/notagent-tui/src/components/cancellable_loader.rs` daraufhin zu
   prüfen und das Ergebnis im Kommentar des Zeilentyps festzuhalten.

8. **Die Abweichung von der TS-Vorlage wird später als Portierungsfehler
   gelesen.**
   Mitigation: Schritt 10 trägt sie in `CONVENTIONS.md` und in den Dateiköpfen
   nach, zusammen mit den drei Aufbau-Unterschieden zur Referenz.

## Alternative Approaches

1. **Nur die Schritte 1 bis 4 umsetzen und den Typwechsel lassen.** Beseitigt
   zwei der drei Allokationen pro Zeile, lässt den Bytevergleich im Diff
   unangetastet und ändert keine Signatur. Deutlich geringeres Risiko, geringerer
   Gewinn. Zugleich die Rückfallposition, die Schritt 9 ausdrücklich vorsieht.

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
