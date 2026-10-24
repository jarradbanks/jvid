# Глоссарий: русский (ru)

The Russian translation of each [glossary](glossary.md) term, used every time the term appears
in the `ru.toml` catalogs. Each choice follows what Russian-speaking editors see in the
localized DaVinci Resolve and Premiere Pro; where those two disagree, the note says which one
jvid follows and why. See [Translating jvid](../translating.md).

## Conventions

- **Capitalization**: labels, buttons, menu items, tabs and headings use sentence case, as in
  Russian Windows and macOS ("Сохранить как", "Очередь рендеринга"), never English Title Case.
- **Buttons and menu items** use the infinitive, as native Russian apps do ("Сохранить",
  "Отменить", "Экспортировать"). Instructions and hints addressed to the user use the polite
  imperative ("Перетащите клипы сюда", "Выберите клип").
- **Toggles named by a noun** keep the noun ("Привязка", "Связанное выделение"), as in
  Premiere's Timeline menu.
- **Quotation marks**: «ёлочки».
- **ё** is written where the word has it ("ещё", "вперёд"), as macOS does.
- **Units**: МБ, ГБ, КБ, дБ, с, пикс., кадр/с, with a space before the unit. LUFS, codec and
  format names (H.264, HEVC, WAV), keycaps and shortcut notation stay as written.
- **Fixed widths**: where a full word can't fit its fixed width, it is shortened with a period
  as Russian UIs do ("Непрозр.", "Насыщ.", "Темпер.", "Выравн." in the Inspector's 72-point
  label column; "Полн." in the preview quality menu; "Вертик." on the size card). Keyframe
  lanes have room and keep the full word ("непрозрачность").
- **Dates**: day before month, CLDR's abbreviated genitive months ("24 окт.", "24 окт. 2026 г.").
- **Plurals**: `one` (1, 21, 101…), `few` (2–4, 22–24…), `many` (0, 5–20, 11–14…), and `other`
  (fractions, required) written as the genitive singular, the same as `few`.

## Terms

| Term | Russian | Resolve | Premiere Pro | Note |
| --- | --- | --- | --- | --- |
| Project | проект | Проект | Проект | Same everywhere. |
| Media | медиафайлы; panel title «Медиа» | Медиа (page), Медиапул | медиаданные, медиафайлы | «Медиа» is indeclinable and short, so it names the panel and tabs, as Resolve's Media page does. In sentences jvid says «медиафайлы», the word Adobe's Russian help uses ("Организация медиафайлов"). |
| Bin | медиапул | Медиапул | панель «Проект», подборка | jvid's bin is the whole panel, which is Resolve's Media Pool; Premiere's «подборка» is a folder inside the Project panel, a narrower thing. |
| Clip | клип | Клип | Клип | Same everywhere. |
| Track | дорожка | Дорожка | Дорожка | Same everywhere. |
| Timeline | таймлайн | Таймлайн | Временная шкала | Premiere's Russian help says «Временная шкала»; Resolve and Russian editors in general say «таймлайн». jvid follows Resolve: it is one word, which matters on tabs and in long action names, and «временная шкала» would make every timeline action two words longer. |
| Playhead | указатель воспроизведения; «указатель» where the context is the timeline | Указатель воспроизведения | Индикатор текущего времени | Premiere's «индикатор текущего времени» is long and names the time readout as much as the line; jvid follows Resolve. |
| Source | источник | Источник | Монитор источника | Same word; jvid's panel is just «Источник». |
| Program | программа | Таймлайн (viewer) | Монитор программы | Resolve calls this viewer after the timeline; jvid follows Premiere, whose «программа» is the term for this viewer. |
| Viewer | монитор | Вьюер, окно просмотра | Монитор | Resolve's «вьюер» is a loan word; Premiere's «монитор» is the established Russian term. |
| Inspector | инспектор | Инспектор | Элементы управления эффектами | Premiere has no Inspector panel; jvid follows Resolve. |
| Effect | эффект | Эффект | Эффект | Same everywhere. |
| Transition | переход | Переход | Переход | Same everywhere. |
| Keyframe | ключевой кадр | Ключевой кадр | Ключевой кадр | Same everywhere. |
| Marker | маркер | Маркер | Маркер | Same everywhere. |
| In point | точка входа; «вход» in short labels | Точка входа | Точка входа, «Вход» | Same everywhere; Adobe's help shortens it to «Вход». |
| Out point | точка выхода; «выход» in short labels | Точка выхода | Точка выхода, «Выход» | As above. |
| Range | диапазон | Диапазон | Диапазон | Same everywhere. |
| Trim | подрезка, подрезать | Подрезка | Обрезка | Premiere's «обрезка» is also its name for Crop, which jvid shows next to trimming; jvid follows Resolve's «подрезка» and keeps «обрезка» for Crop. |
| Ripple | со сдвигом | со сдвигом (Ripple Delete: «Удалить со сдвигом») | со сдвигом (Ripple Delete: «Удаление со сдвигом») | Both say «со сдвигом»; jvid uses the infinitive form of the command. |
| Roll | перемещение склейки | not confirmed | Прокрутка (Rolling Edit), not confirmed | No catalog text uses it yet. «Прокрутка» would clash with scrubbing; the descriptive term says what moves. Recheck against both apps when a string first needs it. |
| Slip | сдвиг содержимого | not confirmed | Скольжение (Slip), not confirmed | No catalog text uses it yet. «Скольжение» alone is easily confused with Slide; recheck when a string first needs it. |
| Split | разделить | Разделить клип | Добавить точку редактирования | Premiere names the command after the edit point it adds; Resolve's «разделить» says what happens to the clip. The Razor tool is «Лезвие» in both. |
| Snapping | привязка | Привязка | Привязка | Adobe's help: «Привязка к временной шкале (S)». |
| Link | связать; Linked Selection «Связанное выделение» | Связать клипы; Связанный выбор | Связать; Связанное выделение | jvid follows Premiere's «выделение»: «выбор» in Resolve reads as choosing an option. |
| Lock | заблокировать, блокировка | Заблокировать дорожку | Заблокировать дорожку | Same everywhere. |
| Mute | без звука | Без звука | Отключить звук дорожки | The track button is a short toggle; «Без звука» fits it and Resolve uses it. |
| Solo | соло | Соло | Соло | Same everywhere. |
| Scrubbing | прокрутка | Скраббинг | Прокрутка звука | Premiere's «прокрутка» is Russian and needs no explanation; Resolve keeps the English loan word. |
| Shuttle | челнок, челночная перемотка | Челнок (JKL) | Челночная перемотка | Action names use the short «челнок» ("Челнок назад"). |
| Timecode | таймкод | Таймкод | Таймкод | Same everywhere. |
| Frame rate | частота кадров | Частота кадров | Частота кадров | Unit: «кадр/с». |
| Resolution | разрешение | Разрешение | Разрешение | Same everywhere. |
| Aspect ratio | соотношение сторон | Соотношение сторон | Соотношение сторон | Same everywhere. |
| Proxy | прокси | Прокси | Прокси | Indeclinable in both. |
| Render | рендеринг | Рендеринг, рендер | Рендеринг | Same everywhere. |
| Export | экспорт, экспортировать | Экспорт | Экспорт | Same everywhere. |
| Render Queue | очередь рендеринга | Очередь рендеринга | — (Adobe Media Encoder: «Очередь») | Premiere hands the queue to Media Encoder; jvid follows Resolve. |
| Preset | пресет | Пресет | Стиль | Adobe's Russian UI says «стиль» («Импорт и экспорт стиля»), which jvid would confuse with a text style; Resolve's «пресет» is what Russian editors say. |
| Codec | кодек | Кодек | Кодек | Names stay as written. |
| Bitrate | битрейт | Битрейт | Скорость передачи данных | Premiere's phrase is long; «битрейт» is Resolve's and the common term. |
| Title | титр | Титр | Титр | Same everywhere. |
| Subtitles | субтитры | Субтитры | Субтитры | Same everywhere. |
| Waveform | форма волны | Форма волны | Форма волны | The video scope of the same English name is «Осциллограф», the Russian name for a waveform monitor. |
| Mixer | микшер | Микшер | Микшер дорожек | Same word. |
| Voiceover | закадровый голос | Закадровый голос | Закадровый голос | Same everywhere. |
| Undo / Redo | отменить / повторить | Отменить / Повторить | Отменить / Повторить | Windows and macOS Russian use the same pair. |

## Sources checked

Adobe's Russian help confirms «Временная шкала», «Монитор источника», «Вход»/«Выход» and
«Привязка к временной шкале (S)»
([source monitor](https://helpx.adobe.com/ru/premiere/desktop/get-started/source-and-program-monitor-adjustments/set-in-and-out-points-in-the-source-monitor.html),
[snap clips](https://helpx.adobe.com/ru/premiere/desktop/edit-projects/change-clip-sequence/snap-clips.html))
and «стиль» for presets ([custom presets](https://helpx.adobe.com/ru/premiere/desktop/render-and-export/export-files/create-and-save-custom-presets.html)).
Russian Resolve material says «Медиа-пул» ([ichip.ru](https://ichip.ru/sovety/videoredaktor-davinci-resolve-bazovaya-informaciya-dlya-nachinayushhikh-272502)).
Adobe's help pages refuse automated fetching and Blackmagic's `/ru/` product pages are in
English, so the remaining Resolve and Premiere labels in the table come from the localized
applications as known to the translator, not from a page checked during this translation. A
native-speaker review of the running editor is requested in the pull request.
