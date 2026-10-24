# Glossary: Danish (da)

One Danish word for each [glossary](glossary.md) term, used everywhere the term appears in the
`da.toml` catalogs. Neither DaVinci Resolve nor Premiere Pro ships a Danish interface, so the
words follow Shotcut's Danish translation where it has one, then the words Danish editors and
Danish software (Windows, macOS) use. Terms marked *unconfirmed* have no localized editor to
check against and want a native editor's eye.

| Term | Dansk | Shotcut | Note |
| --- | --- | --- | --- |
| Project | Projekt | Projekt | |
| Media | Medier | | The bin panel is titled Medier; running text says "mediepanelet" where English says "media bin". |
| Bin | mediepanel | | *Unconfirmed.* No Danish word for a bin exists in a shipped editor; the panel's own name avoids the loanword. |
| Clip | Klip | Klip | Neuter: et klip, klippet, flere klip, klippene. |
| Track | Spor | Spor | Neuter: et spor, sporet, sporene. |
| Timeline | Tidslinje | Tidslinje | |
| Playhead | Afspilningshoved | | *Unconfirmed*; the common Danish word in editing tutorials. |
| Source | Kilde | Kilde | |
| Program | Program | | |
| Viewer | Fremviser | | Rarely shows; the panels are titled Kilde and Program. |
| Inspector | Inspektør | | Danish spelling of Resolve's panel name. |
| Effect | Effekt | Filtre | Shotcut calls its effects filters; jvid's word is effect. |
| Transition | Overgang | Overgang | |
| Keyframe | Keyframe | Keyframe | Common gender, plural "keyframes". |
| Marker | Markør | Markør | |
| In point | Ind-punkt | | *Unconfirmed*; menus say "Markér ind", "Gå til ind", as English's short forms. |
| Out point | Ud-punkt | | As Ind-punkt. |
| Range | Område | | |
| Trim | Trimme | | |
| Ripple | Ripple | | Kept as the trim mode's name, as editors say it. Ripple Delete is "Slet og luk hul". |
| Roll | Rulle | | *Unconfirmed.* |
| Slip | Slip | | |
| Split | Opdele | | "Opdel ved afspilningshoved"; "Del" would read as Share. |
| Snapping | Snap | Snap | |
| Link | Sammenkæde | | "Sammenkædet markering" for Linked Selection. |
| Lock | Lås | | |
| Mute | Lydløs | Lydløs | |
| Solo | Solo | | |
| Scrubbing | Scrubbing | Scrub | "Lydscrubbing" for Audio Scrubbing. |
| Shuttle | Shuttle | | |
| Timecode | Tidskode | Tidskode | |
| Frame rate | Billedfrekvens | Billedfrekvens | |
| Resolution | Opløsning | Opløsning | |
| Aspect ratio | Billedformat | Højde-/breddeforhold | The everyday Danish word; Shotcut's is a literal description. |
| Proxy | Proxy | Proxy | Plural "proxyer". |
| Render | Rendere | | Noun "rendering". |
| Export | Eksportér | Eksportér | Noun "eksport". |
| Render Queue | Renderkø | | *Unconfirmed.* |
| Preset | Forudindstilling | Forudindstilling | Also Microsoft's word. |
| Codec | Codec | Codec | Codec names stay as written. |
| Bitrate | Bithastighed | Bithastighed | |
| Title | Titel | | |
| Subtitles | Undertekster | | |
| Waveform | Bølgeform | | |
| Mixer | Mixer | | |
| Voiceover | Speak | | The Danish broadcast word; "Optag speak". |
| Undo / Redo | Fortryd / Gentag | Fortryd / Omgør | Windows and macOS Danish say Gentag. |

## Conventions

- **Capitalization**: sentence case; only the first word and proper names take a capital
  ("Nyt projekt", "Tilføj markør").
- **Buttons and menu items** use the imperative, as Danish Windows and macOS do ("Gem",
  "Åbn", "Importér"); -ere verbs take the accent ("Eksportér", "Kopiér"), except "Annuller", as Windows writes it.
- **Messages addressed to the user** use "du".
- **Quotation marks** are »…«.
- **Dates**: lowercase month abbreviations with a period ("24. okt.").

## Sources

- Shotcut's Danish translation, `translations/shotcut_da.ts` in
  <https://github.com/mltframework/shotcut>: Tidslinje, Spor, Klip, Keyframe, Markør, Snap,
  Løft, Overskriv, Indsæt, Lydløs, Kilde, Proxy, Eksportér, Overgang, Fortryd, Omgør,
  Forudindstilling, Billedfrekvens, Opløsning, Bithastighed, Tidskode, Overtoning.

## Words kept apart

- Paste is "Sæt ind", as in Danish Windows, so Insert can be "Indsæt".
- Reverse (playback) is "Baglæns"; "Invertér" is only the mask's Invert.
- Lossless export's Copy mode is "Streamkopi", so it never reads as Kopiér.
- An edit point is "klippepunkt"; "klip" alone is always a clip.
