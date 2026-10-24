# Glossary: Polish (pl)

One Polish term per [glossary](glossary.md) entry, used everywhere the term appears in the
`pl.toml` catalogs.

## Sources

Neither reference editor ships a Polish interface: DaVinci Resolve's interface languages are
English, Spanish, Japanese, Simplified Chinese, French, Portuguese, Russian, Korean, Thai and
Vietnamese ([Wikipedia, DaVinci Resolve](https://en.wikipedia.org/wiki/DaVinci_Resolve)), and
Premiere Pro has no Polish interface either (Polish users on the Adobe forums still ask for
Polish transcription, not the UI, e.g.
[community.adobe.com](https://community.adobe.com/t5/premiere-pro-ideas/speech-to-text-polish/idc-p/14927699)).
Final Cut Pro isn't localized into Polish (iMovie is). Polish editors therefore mostly work in
the English UI, and the Polish words they know come from:

1. **Blackmagic's own Polish pages** for Resolve, the closest thing to localized Resolve:
   [Edit](https://www.blackmagicdesign.com/pl/products/davinciresolve/edit),
   [Cut](https://www.blackmagicdesign.com/pl/products/davinciresolve/cut) and
   [What's new](https://www.blackmagicdesign.com/pl/products/davinciresolve/whatsnew), checked
   October 2026. Called **BMD** below.
2. **Kdenlive's Polish catalog** (`po/pl/kdenlive.po` in the
   [24.08.3 source release](https://download.kde.org/stable/release-service/24.08.3/src/)),
   called **Kdenlive**.
3. **Shotcut's Polish catalog**
   ([`translations/shotcut_pl.ts`](https://github.com/mltframework/shotcut/blob/master/translations/shotcut_pl.ts)),
   called **Shotcut**.
4. Polish Windows/macOS and Office conventions for generic commands (Plik, Edycja, Cofnij,
   Ponów, Zaznacz wszystko), and Photoshop's Polish UI for blend modes, opacity and feather.

Where these disagree, BMD wins, unless its word collides with another meaning in Polish UI;
then the next source wins and the row says why.

## Terms

| Term | Polish | Notes |
| --- | --- | --- |
| Project | projekt | All sources. |
| Media | multimedia | BMD "pula multimediów", Kdenlive "Multimedia". The panel, its tab and the prose use **multimedia** ("Upuść tu multimedia", "Eksportuj multimedia"); the adjective is "multimedialny" ("plik multimedialny"). |
| Bin | panel Media | BMD says **kosz**, Kdenlive **pojemnik**. In Polish Windows "Kosz" is the Recycle Bin, and next to "Usuń" it reads as the trash; jvid's bin panel is titled Multimedia anyway, so it's named by that title: "Pokaż w panelu Multimedia". |
| Clip | klip | BMD, Shotcut. Kdenlive's "wycinek" is its own. |
| Track | ścieżka | All sources. Because "ścieżka" is taken, a mask's path is **krzywa**. |
| Timeline | oś czasu | All sources. |
| Playhead | wskaźnik odtwarzania | BMD. Shotcut and Kdenlive say "głowica odtwarzania/odtwarzająca" (tape-deck heads). Short form in actions: "wskaźnik" ("Podziel przy wskaźniku"). |
| Source | Źródło | BMD "okno montażowe źródła", Kdenlive "Źródło". |
| Program | Program | BMD calls it "okno montażowe osi czasu", which is too long for a tab; Program is Premiere's name and reads naturally in Polish. |
| Viewer | podgląd | BMD "okno montażowe" (long). jvid's dock panel is "Podgląd" (Kdenlive, Shotcut). |
| Inspector | Inspektor | BMD. |
| Effect | efekt | All sources. |
| Transition | przejście | All sources. |
| Keyframe | klatka kluczowa | BMD, Kdenlive. Shotcut's "kluczowa klatka" is the less common order. |
| Marker | znacznik | All sources. |
| In point | punkt wejścia | BMD. Short form where a field is labelled: "Wejście". Kdenlive "punkt początku". |
| Out point | punkt wyjścia | BMD. Short form "Wyjście". Because "Wyjście" is the Out field, the export dialog's Output is **Plik wyjściowy**. |
| Range | zakres | Kdenlive, Shotcut. |
| Trim | przycinanie, verb przytnij | BMD, Shotcut. A clip's crop is **kadrowanie** (Photoshop), so the two don't clash. |
| Ripple | z przesunięciem | BMD leaves "ripple" in English ("Nadpisanie metodą ripple"); Kdenlive's "Zafalowanie" and Shotcut's "Powiąż" are literal or wrong. "Usuń z przesunięciem" says what happens: later clips move. |
| Roll | przesunięcie cięcia | BMD "przesuwanie punktu montażowego". Not shown in jvid yet. |
| Slip | poślizg | Kdenlive. BMD's "przewijanie" is also the Polish word for scrolling and scrubbing. Not shown in jvid yet. |
| Split | podziel | Shotcut "Podziel w miejscu suwaka", BMD "podzieli klip". |
| Snapping | przyciąganie | Shotcut; also the Polish Office/Windows word for snap. |
| Link | połącz / rozłącz | Shotcut. Linked Selection is "Zaznaczanie połączonych". |
| Lock | zablokuj / blokada | BMD "blokowanie", Shotcut "Zablokuj ścieżkę". |
| Mute | wycisz | Kdenlive, Shotcut. BMD's console page leaves "Mute" in English. |
| Solo | solo | Kdenlive "Tryb solo"; BMD leaves "Solo". |
| Scrubbing | przewijanie | Kdenlive's "szorowanie" is a literal calque; Shotcut "przeszukuj dźwięk". "Dźwięk przy przewijaniu" for Audio Scrubbing. |
| Shuttle | szybkie przewijanie, verb przewiń szybko | Kdenlive keeps "Jog Shuttle" for the device; the J/K/L actions are imperatives like every other command ("Przewiń szybko wstecz", "Zatrzymaj szybkie przewijanie"). |
| Timecode | kod czasowy | BMD, Shotcut. Kdenlive "kod czasu". |
| Frame rate | liczba klatek na sekundę | BMD "klatek na sekundę". Label "Klatki na sekundę", unit "kl./s" (Kdenlive "kl/s"). Kdenlive "szybkość klatkowa" and Shotcut "częstotliwość klatek" are less common. |
| Resolution | rozdzielczość | All sources. |
| Aspect ratio | proporcje obrazu | Shotcut "Proporcje", "Format obrazu". Kdenlive's "współczynnik kształtu" is technical. Short label "Proporcje". |
| Proxy | proxy, plik proxy | Shotcut; what Polish editors say. Kdenlive's "wycinki pośrednie" is its own. |
| Render | renderowanie | BMD. Kdenlive's "składanie" is its own. |
| Export | eksport, verb eksportuj | All sources. |
| Render Queue | kolejka renderowania | Follows Render. |
| Preset | preset | BMD "presety". Kdenlive "nastawa", Shotcut "zapisane ustawienia". |
| Codec | kodek | Names stay as written. |
| Bitrate | przepływność | Shotcut; the Polish telecom term. Kdenlive "szybkość bitowa". |
| Title | tytuł | Kdenlive, Shotcut. BMD's "nagłówek" means a heading in Polish UI. A lower third is **belka**, the Polish broadcast word, which BMD also uses ("dolne belki"). |
| Subtitles | napisy | All sources. |
| Waveform | kształt fali | BMD "kształt fali dźwiękowej", Kdenlive "Kształt fali". Also the waveform scope; the Scopes tab is **Wykresy** (Shotcut's "Wskaźniki" would clash with the playhead). |
| Mixer | mikser | BMD, Kdenlive. |
| Voiceover | lektor, nagranie lektora | Shotcut "lektor"; the Polish word for the narrator. "Nagraj lektora". |
| Undo / Redo | Cofnij / Ponów | Polish Windows and Office, Shotcut. macOS says "Powtórz" for Redo; Ponów is the more common in editors. |

## Conventions

- Labels, buttons, menus and tabs use sentence case ("Zapisz jako", "Kolejka renderowania"),
  as Polish Windows and macOS do; English Title Case isn't copied.
- Buttons and menu commands are imperatives ("Zapisz", "Otwórz…", "Dodaj znacznik", "Przejdź o
  klatkę wstecz"), as in Polish Windows and macOS menus; status words, toggles and section heads
  are nouns or participles ("Przyciąganie", "Odtwarzanie w pętli").
- An effect's bypass is "Wyłącz", the pair of its "Włącz".
- Blend modes follow Photoshop's Polish UI (Normalny, Pomnóż, Nakładka, Ciemniej, Jaśniej,
  Różnica, Mnożenie odwrotne for Screen).
- Wording is never shortened or abbreviated to fit a layout; text that overflows is a layout
  bug, fixed in the UI (#310).
- Quotation marks are „…”. The decimal point stays a point, because code formats numbers.
- Plurals use `one` (1), `few` (2–4, 22–24, ..) and `many` (0, 5–21, 25–31, ..); `other`
  repeats `many`. Phrases like "każdy z {count} klipów" are genitive plural for every count
  above one, so their `few` and `many` match.
- "Audio" and "wideo" are used as they are in Polish editing software (Shotcut, BMD).
