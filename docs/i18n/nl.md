# Woordenlijst: Nederlands (nl)

The Dutch translation of each [glossary](glossary.md) term, used every time the term appears in
the `nl.toml` catalogs. See [Translating jvid](../translating.md).

Neither DaVinci Resolve nor Premiere Pro ships a Dutch interface: Resolve's languages are
English, Chinese, Japanese, Spanish, Portuguese, French, Russian, Thai, Vietnamese and Korean, and
Adobe localizes Premiere Pro (and its help PDFs) into English, German, Spanish, French, Italian,
Japanese and Korean, among others, but not Dutch. Final Cut Pro has no Dutch interface either.
Dutch-speaking editors therefore see those two in English, and see Dutch in:

- **Adobe Premiere Elements**, Premiere Pro's consumer sibling, which Adobe does translate into
  Dutch with the same terminology department ("Adobe" below);
- **Kdenlive**, whose Dutch translation is maintained by the KDE Dutch team ("Kdenlive" below).

Where those two agree, jvid uses their word. Where they disagree, or where one translates a term
the other leaves out, the note says which jvid follows and why. Where both translate a term into
something Dutch editors don't actually say, jvid keeps the English loan word they say instead, as
Resolve and Premiere show it.

## Conventions

- **Capitalization**: labels, buttons, menu items, tabs and headings use sentence case
  ("Opslaan als", "Renderwachtrij tonen"), as Dutch Windows, macOS, Adobe and KDE do; never
  English Title Case. Names of other interface elements inside a sentence keep their capital
  ("klik op Importeren", "het tabblad Effecten").
- **Commands** (buttons, menu items, action names) use the infinitive at the end ("Project
  openen", "Markering toevoegen", "Ongedaan maken"), as Dutch Windows, Adobe and KDE do. Dutch
  macOS uses the imperative ("Bewaar", "Open"); jvid follows Windows because it is the larger
  audience and because Adobe and Kdenlive, the two Dutch editors, use the infinitive. Navigation
  actions are short prepositional phrases ("Naar inpunt", "Naar volgende markering"), as in
  Kdenlive.
- **Instructions and hints** addressed to the user use the imperative ("Sleep clips hierheen",
  "Selecteer eerst clips op de tijdlijn").
- **Form of address**: informal *je*, as current Dutch Windows, macOS and Google products do.
  Adobe's and KDE's older *u* reads stiff in a new app.
- **Quotation marks**: “…” (the English catalogs' curly quotes), standard in Dutch typesetting.
- **Compounds** are written as one word ("renderwachtrij", "tijdlijn", "videotrack"), with a
  hyphen where vowels collide ("audio-effecten") or before a code or abbreviation ("HDR-uitvoer",
  "H.264- en HEVC-voorinstellingen", "jvid-clip", "LUT-mix").
- **Units**: a space before the unit (`{value} MB`, `{seconds} s`, `{n} px`); `%` follows the
  number directly, as Dutch Windows writes it. LUFS, fps, codec and format names, keycaps and
  shortcut notation stay as written.
- **Dates**: day before month, CLDR's abbreviated months ("24 okt.", "24 okt. 2026", "3 mei").
- **Plurals**: `one` for 1 and `other` for everything else, as in English. "uur" doesn't take a
  plural after a number ("2 uur geleden").
- **Widths**: the Inspector's label column is 72 points; every Dutch caption that goes there
  is a single short word ("Dekking", "Verzadiging", "Eindgain"). "Temperatuur" is 0.4 pt wider
  than English "Temperature", which already fills the column; it still shows whole. Keyframe
  lanes have room and use the full compound ("Uitsnijden links", "Omlijningsbreedte"). Fixed-width
  dropdowns forced three short forms: preview quality "Vol" (not "Volledig"), bin sort "Datum"
  (not "Toegevoegd") and blend mode "Vermenigv." (not "Vermenigvuldigen"). The Export dialog's
  queue button reads "In wachtrij", because "Aan wachtrij toevoegen" ran into the size estimate.

## Terms

| Term | Dutch | Adobe (Premiere Elements nl) | Kdenlive nl | Note |
| --- | --- | --- | --- | --- |
| Project | project | project | project | Same everywhere. |
| Media | media | media | Media | Same everywhere; takes a plural verb in sentences ("koppel ze opnieuw"). |
| Bin | mediapaneel | deelvenster Projectelementen | Projectmap | jvid's bin panel is titled "Media", so in sentences it is "het mediapaneel" ("Tonen in mediapaneel"). Adobe's "Projectelementen" and Kdenlive's "Projectmap" name a panel jvid doesn't title that way. |
| Clip | clip (de clip, clips) | clip | Clip | Same everywhere. |
| Track | track (de track, tracks); videotrack, audiotrack | track | Track | Both keep the English word; Adobe uses "spoor" almost never. |
| Timeline | tijdlijn | tijdlijn | Tijdlijn | Same everywhere. |
| Playhead | afspeelkop | huidige-tijdindicator (CTI) | afspeelkop | Kdenlive's word is short and what Dutch editors say; Adobe's is a long literal rendering of "current-time indicator". |
| Source | bron | Bronmonitor | Bron, Clipmonitor | jvid's viewer is "Bron", as in Adobe's Bronmonitor. |
| Program | programma | — (Premiere Pro English: Program Monitor) | Project Monitor | Premiere's Program Monitor → "programma", which reads naturally beside "Bron". Kdenlive leaves "Project Monitor" untranslated. |
| Viewer | monitor | Monitor | Monitor | Same everywhere. |
| Inspector | Inspector | deelvenster Eigenschappen | — (Effect Stack) | Resolve's and Final Cut's panel name, and what Dutch editors call it; it is also a Dutch word. Adobe's "Eigenschappen" would clash with the panel's own "properties" sentence. |
| Effect | effect (het effect, effecten) | effect | Effect | Same everywhere. |
| Transition | overgang | overgang | Overgang | Same everywhere. A crossfade is an "overvloeier", the Dutch film term; dips are "via zwart" / "via wit". |
| Keyframe | keyframe (het keyframe) | hoofdframe | keyframe | Kdenlive's loan word is what Dutch editors use in practice (and see in English Resolve and Premiere); Adobe's "hoofdframe" is rarely heard. |
| Marker | markering | markeerteken | Markering | Kdenlive's word is shorter and the everyday one; Adobe's "markeerteken" (marking sign) fits badly in action names ("Naar volgende markering"). |
| In point | inpunt; "In" in short labels | inpunt | Beginpunt | Adobe's "inpunt"/"uitpunt" pair mirrors the English, so short labels can say "In"/"Uit". Kdenlive's "begin-"/"eindpunt" is clear but loses that pairing. |
| Out point | uitpunt; "Uit" in short labels | uitpunt | Eindpunt | As above. |
| Range | bereik | bereik | Zone | Kdenlive's "zone" is its own feature's name. |
| Trim | bijsnijden | bijsnijden | — | Adobe's word. Crop is "uitsnijden", as in Adobe, so the two never meet. |
| Ripple | doorschuiven | rimpelverwijdering; "Verwijderen en tussenruimte sluiten" | Aaneenvoegen | Neither is usable as a short label: Adobe's "rimpel" is a literal calque only its help uses, and Kdenlive's "aaneenvoegen" (join together) misdescribes it. "Doorschuiven" says what happens to later clips ("Verwijderen en doorschuiven"). |
| Roll | rollen | — | Rollen | Kdenlive's word. No catalog text uses it yet. |
| Slip | slippen | — | Slippen | Kdenlive's word. No catalog text uses it yet. |
| Split | splitsen | splitsen | splitsen | Same everywhere ("Splitsen bij afspeelkop"). The Razor tool is "snijgereedschap" (Kdenlive). |
| Snapping | vastklikken | Magnetisch | Vastklikken | Adobe's "Magnetisch" is an adjective for a menu checkmark and reads oddly as an action name; Kdenlive's verb works both as a toggle and in sentences. |
| Link | koppelen; Linked Selection "Gekoppelde selectie" | koppelen / ontkoppelen | — | Adobe's word. Relink is "opnieuw koppelen". |
| Lock | vergrendelen | vergrendelen | vergrendelen | Same everywhere. |
| Mute | dempen | dempen | Dempen | Same everywhere. |
| Solo | solo | — | — | The audio term, unchanged in Dutch. |
| Scrubbing | scrubben | scrubben | Audioscrubbing | Adobe's Dutch verb ("Scrubben met audio"). |
| Shuttle | shuttle | shuttle (shuttleschuifregelaar) | JogShuttle | Adobe keeps the English word ("Shuttle links"). |
| Timecode | tijdcode | tijdcode | Tijdcode | Same everywhere. |
| Frame rate | framesnelheid | framesnelheid | Framesnelheid | Same everywhere. |
| Resolution | resolutie | resolutie | Resolutie | Same everywhere. |
| Aspect ratio | beeldverhouding; "Verhouding" as a short label | hoogte-breedteverhouding | Beeldverhouding | Kdenlive's word is the common one; Adobe's is long. |
| Proxy | proxy (proxy's) | — | Proxy | Dutch spelling of the plural is "proxy's". |
| Render | renderen | renderen | Renderen | Same everywhere. |
| Export | exporteren; de export (exports) | exporteren | exporteren | Same everywhere. |
| Render Queue | renderwachtrij | — | wachtrij ("opdracht in de wachtrij") | Kdenlive's render jobs wait "in de wachtrij", the Dutch Windows word for a queue; "render-" names which one. |
| Preset | voorinstelling | voorinstelling | Voorinstelling | Same everywhere. |
| Codec | codec | codec | codec | Names stay as written. |
| Bitrate | bitsnelheid | bitsnelheid | Bitsnelheid | Same everywhere. |
| Title | titel | titel | Titel | Same everywhere. A lower third is a "naambalk", the Dutch broadcast term. |
| Subtitles | ondertitels | — | Ondertitels | Kdenlive's word. |
| Waveform | golfvorm | golfvorm | Golfvorm | Same everywhere; also the name of the video scope. |
| Mixer | mixer, audiomixer | audiomixer | Audiomixer | Same everywhere. |
| Voiceover | voice-over | voice-over ("Gesproken commentaar opnemen") | — | The word Adobe's help and Dutch dictionaries use. |
| Undo / Redo | ongedaan maken / opnieuw | Ongedaan maken / Opnieuw | Ongedaan maken / Opnieuw | Same everywhere, and as in Dutch Windows. |

Words outside the glossary that follow an outside source: blend modes as in the Dutch Photoshop
("Vermenigvuldigen", "Bleken", "Bedekken", "Donkerder", "Lichter", "Verschil"); "Doezelaar"
for feather, also Photoshop's; "Beweging verminderen" for Reduce Motion, the setting's name in
Dutch macOS and iOS (there as the imperative "Verminder beweging"); "Uitlichten" and
"Extraheren" for Lift and Extract, which no Dutch editor translates (Kdenlive's "Verhogen" for
Lift means "raise" and is a mistranslation).

## Sources checked

- Kdenlive's Dutch catalog, [`po/nl/kdenlive.po`](https://github.com/KDE/kdenlive/blob/master/po/nl/kdenlive.po)
  (revision of 2026-10-22), searched for each term: Tijdlijn, afspeelkop, Bron, Clipmonitor,
  Overgang, keyframe, Markering, Beginpunt/Eindpunt, Aaneenvoegen, Rollen, Slippen,
  Snijgereedschap, Vastklikken, Dempen, Audioscrubbing, Tijdcode, Framesnelheid,
  Beeldverhouding, Proxy, Voorinstelling, Bitsnelheid, Ondertitels, Golfvorm, Audiomixer,
  Ongedaan maken / Opnieuw, Verhogen (Lift), Projectmap, wachtrij, Ondertiteltrack.
- Adobe's Dutch [Premiere Elements reference](https://help.adobe.com/archive/nl/premiere-elements/12/premiere-elements_reference.pdf),
  searched for each term: tijdlijn, huidige-tijdindicator (CTI), Bronmonitor, inpunt/uitpunt,
  hoofdframe, markeerteken, bijsnijden, Uitsnijden, splitsen, Magnetisch,
  rimpelverwijdering and "Verwijderen en tussenruimte sluiten", koppelen/ontkoppelen, scrubben,
  shuttle, Gesproken commentaar / voice-over, golfvorm, audiomixer, hoogte-breedteverhouding,
  renderen, voorinstelling, bitsnelheid, Projectelementen, Eigenschappen, Stilstaand beeld,
  Ongedaan maken / Opnieuw. Adobe's current Dutch help pages
  ([markers](https://helpx.adobe.com/nl/premiere-elements/desktop/arranging-clips/clip-timeline-markers.html),
  [previewing](https://helpx.adobe.com/nl/premiere-elements/desktop/working-with-projects/previewing-movies.html))
  use the same terms but refuse automated fetching.
- Resolve's interface languages: [Wikipedia](https://en.wikipedia.org/wiki/DaVinci_Resolve) and a
  Dutch tutorial noting Dutch is missing from Resolve's language setting
  ([tutkit.com](https://www.tutkit.com/nl/tekst-tutorials/14269-davinci-resolve-gebruik-van-de-engelse-gebruikersinterface)).
- Premiere Pro's languages: Adobe's [localized help PDF list](https://helpx.adobe.com/se/x-productkb/global/help-pdf-localized-languages.html),
  which has no Dutch Premiere Pro, and an
  [Adobe Community thread](https://community.adobe.com/t5/premiere-elements-discussions/premiere-elements-2024-from-dutch-language-to-english-language/td-p/14695647)
  confirming Premiere Elements installs in Dutch.

Dutch Windows, macOS and Photoshop labels above are from the localized products as known to the
translator, not from a page checked during this translation. A native-speaker review of the
running editor is requested in the pull request.
