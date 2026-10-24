# Glossary: German (de)

One German word for each [glossary](glossary.md) term, used everywhere the term appears in the
`de.toml` catalogs. Each follows the German DaVinci Resolve or Premiere Pro interface; where
the two differ, the note says which one jvid follows and why, with Final Cut Pro as the tie-break.

| Term | Deutsch | DaVinci Resolve | Premiere Pro | Note |
| --- | --- | --- | --- | --- |
| Project | Projekt | Projekt | Projekt | |
| Media | Medien | Medien | Medien | |
| Bin | Ablage | Bin | Ablage | Premiere's German word; Resolve keeps the English "Bin", which reads as a loanword to anyone not already using Resolve. jvid's bin is the Media panel, so "Medienablage" where English says "media bin". |
| Clip | Clip | Clip | Clip | |
| Track | Spur | Spur | Spur | |
| Timeline | Timeline | Timeline | Schnittfenster (panel), Sequenz | Resolve and Final Cut both say Timeline, and it is the word German editors use; Premiere's "Schnittfenster" names its panel, not the idea. |
| Playhead | Abspielkopf | Abspielkopf | Abspielkopf | Final Cut says "Abspielposition". |
| Source | Quelle | Quelle | Quelle (Quellmonitor) | |
| Program | Programm | Programm | Programm (Programmmonitor) | |
| Viewer | Viewer | Viewer | Monitor | Resolve's word; jvid's panels are titled Quelle and Programm, so the term itself rarely shows. |
| Inspector | Inspektor | Inspector (manual), Inspektor (Blackmagic's German site, German tutorials) | Effekteinstellungen | German spelling of Resolve's panel name; Premiere's "Effekteinstellungen" names a panel that holds only effect controls. Its sections are nouns ("Transformation"), since they fill "{section} zurücksetzen". |
| Effect | Effekt | Effekt | Effekt | |
| Transition | Übergang | Übergang | Überblendung | Resolve and Final Cut say Übergang. jvid's transitions include dips, which are not blends, and "Überblendung" is kept for the crossfade itself. |
| Keyframe | Keyframe | Keyframe | Keyframe | Plural "Keyframes". |
| Marker | Marker | Marker | Marke | Resolve and Final Cut agree on Marker; Premiere's "Marke" also means a brand. |
| In point | In-Punkt | In-Punkt | In-Point | Resolve's German compound, used in full in menus and shortcuts ("In-Punkt setzen", "Zum In-Punkt gehen"). Only the Export dialog's range says "In bis Out". |
| Out point | Out-Punkt | Out-Punkt | Out-Point | As In-Punkt. |
| Range | Bereich | Bereich | In/Out-Bereich | |
| Trim | Trimmen | Trimmen | Trimmen | |
| Ripple | Ripple | Ripple (Ripple-Trim, Ripple-Modus) | Ripple, "Löschen und Lücke schließen" | Kept as the trim mode's name, as both do. Ripple Delete is "Löschen und Lücke schließen" in both German interfaces, so jvid uses that phrase for the delete. |
| Roll | Rollen | Rollen | Rollen | |
| Slip | Slip | Slip | Clipinhalt verschieben | Resolve's name is the short one. |
| Split | Teilen | Teilen (zerteilen) | Schnitt hinzufügen | jvid's action is "Split at Playhead", so a verb: "Am Abspielkopf teilen". |
| Snapping | Einrasten | Einrasten | Ausrichten | Resolve and Final Cut say Einrasten; Premiere's "Ausrichten" is also its word for aligning, which jvid's text tools use. |
| Link | Verknüpfen | Verknüpfen | Verknüpfen | "Verknüpfte Auswahl" for Linked Selection, as Premiere. |
| Lock | Sperren | Sperren | Sperren | |
| Mute | Stummschalten | Stummschalten | Stummschalten | The track button shows "M"; its tooltip says "Stummschalten". |
| Solo | Solo | Solo | Solo | Final Cut says "Isolieren". |
| Scrubbing | Scrubbing | Scrubbing | Scrubbing | |
| Shuttle | Shuttle | Shuttle | Shuttle | As Premiere: "Shuttle links", "Shuttle rechts", "Shuttle Stopp". |
| Timecode | Timecode | Timecode | Timecode | |
| Frame rate | Framerate | Framerate | Framerate | |
| Resolution | Auflösung | Auflösung | Auflösung | |
| Aspect ratio | Seitenverhältnis | Bildseitenverhältnis | Seitenverhältnis | The shorter, common form. |
| Proxy | Proxy | Proxy | Proxy | Plural "Proxys", as both. |
| Render | Rendern | Rendern | Rendern | |
| Export | Exportieren | Exportieren | Exportieren | Noun "Export". |
| Render Queue | Renderliste | Render-Queue, Renderwarteschlange (German tutorials; not checked in the app) | Warteschlange (Media Encoder) | No source confirms Resolve says "Renderliste"; it is kept because "Zur Renderwarteschlange" overflows the Export dialog's footer, and it reads plainly as the list of renders. |
| Preset | Vorgabe | Preset | Vorgabe | Adobe's German word across its apps; Resolve leaves "Preset" in English. |
| Codec | Codec | Codec | Codec | Codec names stay as written. |
| Bitrate | Bitrate | Bitrate | Bitrate | |
| Title | Titel | Titel | Titel | |
| Subtitles | Untertitel | Untertitel | Untertitel | |
| Waveform | Wellenform | Wellenform | Wellenform | |
| Mixer | Mixer | Mixer | Audiospur-Mischer | |
| Voiceover | Voiceover | Voiceover | Voiceover ("Voiceover aufzeichnen") | |
| Undo / Redo | Rückgängig / Wiederholen | Rückgängig / Wiederholen | Rückgängig / Wiederherstellen | Windows and macOS German both say Wiederholen for Redo in their own apps' Edit menus. |

## Conventions

- **Capitalization**: German capitalizes nouns and the first word, nothing else. Labels are
  "Neues Projekt", "Marker hinzufügen", never English Title Case.
- **Buttons and menu items** use the infinitive, as Windows and macOS German apps do
  ("Speichern", "Datei importieren", "Marker hinzufügen"); short toggles name the thing
  ("Einrasten", "Verknüpfte Auswahl").
- **Messages addressed to the user** use "Sie", as Resolve and Premiere do.
- **Quotation marks** are „…“.
- **Ellipsis** "…" after a command that opens a dialog, as in English.
- **Numbers** keep the decimal point code formats; the catalogs hold no numbers of their own
  apart from fixed ones like "10-Bit".

## Fit decisions

Words chosen over the obvious one because the obvious one clipped a fixed-width control:

- Speed is "Tempo" in the Inspector and lanes; "Geschwindigkeit" overflows the label column.
- Blend modes follow GIMP ("Bildschirm", "Überlagern"); Photoshop's "Negativ multiplizieren"
  and "Ineinanderkopieren" overflow the blend dropdown. "Füllmethode" names the control.
- Crop is "Zuschnitt", Fit is "Passend", the bin's Date Added sort is "Importiert", the
  vertical size is "9:16 · Vertikal" and Custom is "Eigene Größe".
- The Export dialog says "Zur Renderliste", "Geschätzt", "Ganze Timeline" and "Auswahl" so its
  footer keeps English's width; the cancel prompt's Keep Exporting is "Weiter", next to "Alle abbrechen".
- Insert is "Clip einfügen", so it never reads as Paste ("Einfügen") in the Edit menu.

## Words kept apart

- Reverse (playback) is "Rückwärts" everywhere; "Umkehren" is only the mask's Invert, as in
  Photoshop.
- Where English "frame" means the picture's canvas (Project Settings' rescale and refit
  prompts), German says "Bildformat"; "Frame" is always one picture.
- Lossless export's Copy mode is "Stream-Kopie", so it never reads as Edit > Kopieren.
- Ease In/Out are "Langsamer Eintritt", "Langsamer Austritt" and "Langsamer Ein-/Austritt".
  Neither Resolve's nor Premiere's German wording could be confirmed from a source.

## Sources

- Blackmagic Design, *Der ultimative Leitfaden zu DaVinci Resolve 15* (German),
  <https://documents.blackmagicdesign.com/de/UserManuals/DaVinci-Resolve-15-Definitive-Guide.pdf>:
  Abspielkopf, Einrasten, Bin, Spur, Marker, In-Punkt/Out-Punkt, Ripple, Slip, Trimmen,
  Keyframe, Übergang, Viewer, Timeline, Preset, Framerate, Bildseitenverhältnis,
  stummschalten, Solo, verknüpfen, Scrubbing, Wellenform, Mixer, Bitrate; and
  "Edit > Ripple Delete (Bearbeiten > Löschen und Lücke schließen)".
- Blackmagic Design, DaVinci Resolve media page (German),
  <https://www.blackmagicdesign.com/de/products/davinciresolve/media>: "Inspektor", "Abspielkopf".
- Sven Brencher, Premiere Pro keyboard shortcuts from the German interface,
  <https://www.svenbrencher.de/download/PremiereProTastaturbefehle.pdf>: Ablage, Marke
  hinzufügen, In setzen/Out setzen, Zu In gehen, In-Point löschen, Herausnehmen, Extrahieren,
  Überlagern, Einfügen, Löschen und Lücke schließen, Schnitt hinzufügen, Rasierklinge,
  Verknüpfte Auswahl, In Schnittfenster ausrichten, Verschachteln, Shuttle - links, Endlosschleife,
  Frame exportieren, Voiceover aufzeichnen, Proxys ein/aus, Rückgängig/Wiederherstellen,
  Effekteinstellungen, Audiospur-Mischer, Quellmonitor, Programmmonitor.
- German DaVinci Resolve tutorials: tutkit.com,
  <https://www.tutkit.com/de/text-tutorials/1762-effiziente-videobearbeitung-mit-davinci-resolve-schritt-fuer-schritt>
  ("Inspektor"), and Lazi Akademie,
  <https://www.lazi-akademie.de/wiki/software-tools/davinci-resolve/deliver-page-davinci-resolve/>
  ("Render-Queue").
- Apple, Final Cut Pro keyboard shortcuts (German),
  <https://support.apple.com/de-de/guide/final-cut-pro/ver90ba5929/mac>: Abspielposition,
  Einrasten, Überschreiben, Einfügen, Standardübergang, Isolieren, Clip aktivieren/deaktivieren.
