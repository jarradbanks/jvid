# Glossario italiano

The Italian translation of the [glossary](glossary.md): one word per term, used everywhere the
term appears in the `it.toml` catalogs. Each choice follows what Italian-speaking editors see in
the localized Premiere Pro, with DaVinci Resolve's Italian pages, Final Cut Pro and Kdenlive
deciding where Premiere lacks a term or reads oddly. See [Translating jvid](../translating.md).

Resolve's interface itself isn't localized into Italian; Blackmagic's Italian site keeps most
editing terms in English (Inspector, playhead, Ripple, Roll, Slip), so its column shows those
pages' wording.

| Term | Italiano | Resolve (Italian site) | Premiere Pro | Why |
| --- | --- | --- | --- | --- |
| Project | progetto | progetto | progetto | |
| Media | media | Media Pool | media | Premiere's Esporta media, Browser media; invariable. |
| Bin | contenitore | cartella (bin) | contenitore, raccoglitore | Premiere's help uses both; Kdenlive's Contenitore del progetto settles it. The panel itself is titled Media, as in jvid's English. |
| Clip | clip (f.) | clip | clip | Invariable: una clip, tre clip. |
| Track | traccia | traccia | traccia | |
| Timeline | timeline (f.) | timeline | timeline | Kdenlive's linea temporale is the one outlier. |
| Playhead | testina di riproduzione | playhead | testina di riproduzione | Short labels say testina. Kdenlive says indicatore di riproduzione. |
| Source | Sorgente | viewer delle fonti | Monitor sorgente | The panel title alone, as in jvid's English. |
| Program | Programma | viewer della timeline | Monitor programma | The panel title alone. |
| Viewer | monitor | viewer | monitor | Premiere's Italian word; Resolve keeps English. |
| Inspector | Inspector | Inspector | Controllo effetti | Resolve and Final Cut Pro's Italian guides keep Inspector; Premiere's panel is a different, effects-only one. |
| Effect | effetto | effetto | effetto | |
| Transition | transizione | transizione | transizione | |
| Keyframe | fotogramma chiave | fotogrammi chiave (keyframe) | fotogramma chiave | Plural fotogrammi chiave. All three agree. |
| Marker | marcatore | marcatore | marcatore | |
| In point | punto di attacco | punto In | attacco | Premiere's Contrassegna attacco; Kdenlive's zona di attacco. Short labels say Attacco. |
| Out point | punto di stacco | punto Out | stacco | Premiere's Contrassegna stacco. Short labels say Stacco. |
| Range | intervallo | intervallo | intervallo | |
| Trim | rifilare | trimming | rifilare | The verb Italian editors use; Resolve keeps English. |
| Ripple | con increspatura | Ripple | con increspatura | Premiere's Elimina con increspatura; Resolve keeps Ripple and Kdenlive says con scarto. The one term a native reviewer should weigh most. |
| Roll | rotazione | Roll | modifica con rotazione | Premiere's term; jvid only names it in docs. |
| Slip | scorrimento | Slip | scorrimento | Premiere's Strumento Scorrimento; Kdenlive says scivolamento. |
| Split | dividere | taglio (blade) | aggiungi modifica | Dividi alla testina says what happens; Premiere's Aggiungi modifica reads like a different command. |
| Snapping | aggancio | aggancio | Aggancia nella timeline | Premiere's toggle; Kdenlive's bordi magnetici is the outlier. |
| Link | collegare | collegare | collega/scollega | Linked Selection is Selezione collegata. |
| Lock | bloccare | bloccare | blocca traccia | |
| Mute | disattivare l'audio | mute | disattiva audio | Premiere's phrase; the track button's tooltip says Disattiva audio. Kdenlive says Silenzia. |
| Solo | solo | solo | solo | |
| Scrubbing | scrubbing | scrubbing | scrubbing audio | The English loan Italian editors use; Kdenlive's pulizia dell'audio means cleaning. |
| Shuttle | shuttle | shuttle (JKL) | shuttle | The broadcast term (jog/shuttle), kept in English everywhere. |
| Timecode | timecode | timecode | timecode | Kdenlive's codice temporale is the outlier. |
| Frame rate | frequenza fotogrammi | fotogrammi al secondo | frequenza fotogrammi | Premiere's label; the unit stays fps. |
| Resolution | risoluzione | risoluzione | risoluzione | |
| Aspect ratio | proporzioni | proporzioni | proporzioni | Kdenlive says rapporto d'aspetto; proporzioni is shorter and Premiere's. |
| Proxy | proxy | proxy | proxy | Invariable. Kdenlive's clip rappresentative is the outlier. |
| Render | rendering | rendering | rendering | Esegui il rendering; as a status, In rendering. |
| Export | esportare | esportare | esporta | Esporta media for the action, as in Premiere. |
| Render Queue | coda di rendering | coda di rendering | Coda (Media Encoder) | Premiere hands its queue to Media Encoder's Coda; coda di rendering says which queue. |
| Preset | predefinito | predefinito | predefinito | Premiere's Predefinito menu (Corrispondenza origine). Plural predefiniti. |
| Codec | codec | codec | codec | Names stay as written. |
| Bitrate | bitrate | bitrate | bitrate | |
| Title | titolo | titolo | titolo | A lower third is a sottopancia, the Italian broadcast term. |
| Subtitles | sottotitoli | sottotitoli | sottotitoli | |
| Waveform | forma d'onda | forma d'onda | forma d'onda | |
| Mixer | mixer | mixer | mixer audio | |
| Voiceover | voce fuori campo | voce fuori campo | voce fuori campo | Premiere's Registra voce fuori campo. |
| Undo / Redo | Annulla / Ripeti | Annulla / Ripeti | Annulla / Ripeti | As in Windows and macOS. |

## Conventions

- Labels, buttons and menu items use sentence case (Salva con nome, Coda di rendering), as
  Windows and macOS do in Italian. Panel titles and proper names keep their capital.
- Commands use the second-person imperative, as Windows and macOS do (Salva, Apri, Esci,
  Annulla, Seleziona tutto); settings and toggles are nouns (Aggancio, Selezione collegata).
  Messages address the user as tu (Seleziona una clip).
- "Couldn't …" errors read Impossibile …, the Windows and macOS phrasing.
- Quotation marks are “curly”, as in macOS's Italian dialogs.
- Dates are day then month, months abbreviated in lower case without a stop (24 ott 2025).
- Plural groups use one, many (exact millions, "1000000 di clip") and other, as CLDR's rule
  for Italian; many adds di only where the count comes straight before a noun.

## Sources checked

- Adobe's Italian Premiere Pro help, [Impostare i punti di attacco e stacco nel monitor
  Sorgente](https://helpx.adobe.com/it/premiere/desktop/get-started/source-and-program-monitor-adjustments/set-in-and-out-points-in-the-source-monitor.html):
  Contrassegna attacco, Contrassegna stacco, Monitor sorgente, testina di riproduzione.
- Adobe's Italian [snap clips](https://helpx.adobe.com/it/premiere/desktop/edit-projects/change-clip-sequence/snap-clips.html):
  Aggancia nella timeline. Its [basic video settings](https://helpx.adobe.com/it/premiere/desktop/render-and-export/export-files/basic-video-settings.html):
  the Predefinito menu and Corrispondenza origine. Its help index: Gestire i contenitori,
  Aggiungere ed eliminare raccoglitori.
- Blackmagic's Italian [Edit page](https://www.blackmagicdesign.com/it/products/davinciresolve/edit):
  Media Pool, cartelle chiamate bin, Inspector, playhead, fotogrammi chiave (keyframe),
  marcatori, punti In e Out, Ripple, Roll, Slip, predefiniti, forma d'onda.
- Kdenlive 21.12's shipped Italian catalog (`kdenlive.mo` from Ubuntu's `kdenlive-data`):
  Contenitore del progetto, Traccia, Marcatore, zona di attacco/stacco, fotogramma chiave,
  indicatore di riproduzione, Blocca traccia, Silenzia traccia, Frequenza dei fotogrammi,
  Rapporto d'aspetto, Sottotitoli, Forma d'onda, Inserisci, Sovrascrivi, Estrai.
- Not confirmed against a live page (Adobe's Italian shortcut list refuses scripted fetches):
  Premiere's Elimina con increspatura, Strumento Scorrimento and Shuttle a sinistra/destra.
