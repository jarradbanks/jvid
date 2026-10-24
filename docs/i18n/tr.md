# Glossary: Turkish (tr)

One Turkish word for each [glossary](glossary.md) term, used everywhere the term appears in the
`tr.toml` catalogs. Neither DaVinci Resolve nor Premiere Pro could be confirmed to ship a Turkish
interface (Adobe's forum has an open request for one), so the words follow Kdenlive and Shotcut,
which do, and the terms Turkish editing tutorials use. Words marked † could not be confirmed
from a localized interface and want a native editor's check.

| Term | Türkçe | Note |
| --- | --- | --- |
| Project | Proje | |
| Media | Medya | |
| Bin | Kutu | Kdenlive's "Proje Kutusu". jvid's bin is the Media panel, so "medya kutusu" where English says "media bin". |
| Clip | Klip | Accusative "klibi", plural "klipler". |
| Track | İz | Kdenlive's word ("Video İzi", "Ses İzi"). Many editors just say "track" or "kanal". |
| Timeline | Zaman Çizelgesi | |
| Playhead | Oynatma Kafası | † |
| Source | Kaynak | |
| Program | Program | |
| Viewer | Görüntüleyici | † Rarely shows; the panels are titled Kaynak and Program. |
| Inspector | Denetçi | † Apple's Turkish word for Inspector. Shotcut's equivalent panel is "Özellikler". |
| Effect | Efekt | |
| Transition | Geçiş | Dips are "Siyaha Geçiş", "Beyaza Geçiş"; Crossfade is "Çapraz Geçiş". |
| Keyframe | Anahtar Kare | |
| Marker | İşaretçi | † |
| In point | Giriş Noktası | "Giriş Noktası Belirle", "Giriş Noktasına Git"; the Export range says "Girişten Çıkışa". |
| Out point | Çıkış Noktası | As Giriş Noktası. |
| Range | Aralık | |
| Trim | Kırpma | Verb "kırp". The Inspector's Crop is also "Kırpma", as image editors say; the two never share a screen. |
| Ripple | Kaydırmalı | † Says what happens (later clips shift): "Kaydırmalı Sil", "Kaydırmalı Kırp". Editors often keep "ripple". |
| Roll | Roll | † Not in the catalogs yet; kept as the loanword. |
| Slip | Slip | † Not in the catalogs yet; kept as the loanword. |
| Split | Böl | "Oynatma Kafasında Böl". |
| Snapping | Yaslama | Adobe's Turkish word for Snap ("Yasla"). |
| Link | Bağla | "Bağlı Seçim" for Linked Selection. |
| Lock | Kilitle | |
| Mute | Sesi Kapat | |
| Solo | Solo | |
| Scrubbing | Tarama | † "Sesli Tarama" for Audio Scrubbing. |
| Shuttle | Shuttle | Kept as the loanword, as the jog/shuttle hardware is sold: "Shuttle Sola", "Shuttle Durdur". |
| Timecode | Zaman Kodu | |
| Frame rate | Kare Hızı | |
| Resolution | Çözünürlük | |
| Aspect ratio | En Boy Oranı | |
| Proxy | Proxy | Plural "proxy'ler". Kdenlive says "vekil", which editors rarely use. |
| Render | Render | "Render etmek", "Render Ediliyor"; the everyday word ("render almak"). |
| Export | Dışa Aktar | Noun "dışa aktarma". |
| Render Queue | Render Kuyruğu | |
| Preset | Ön Ayar | |
| Codec | Kodek | Codec names stay as written. |
| Bitrate | Bit Hızı | |
| Title | Başlık | |
| Subtitles | Altyazı | The Caption text preset is also "Altyazı"; Turkish has one word for both. |
| Waveform | Dalga Biçimi | |
| Mixer | Mikser | |
| Voiceover | Seslendirme | |
| Undo / Redo | Geri Al / Yinele | As Windows and macOS Turkish. |

## Conventions

- **Capitalization**: labels, buttons, menu items and titles capitalize each word except
  "ve", "veya", "ile", "için", "mi", as Windows and macOS Turkish do ("Farklı Kaydet",
  "Tümünü Seç"). Messages, tooltips and errors are sentences. Lowercase i capitalizes to İ.
- **Commands** use the plain imperative ("Kaydet", "İşaretçi Ekle"); instructions to the user use
  the polite one ("bir klip seçin"), as Microsoft's Turkish style does.
- **Placeholders** never take a suffix, since vowel harmony depends on the value. A noun after
  them carries it instead ("{name} projesindeki", "{track} izinde"), and `{clip}` in the lossless
  reasons is followed by none, since it can be "Bir klip".
- **Quotation marks** are “…”. **Ellipsis** "…" after a command that opens a dialog.
- **Units**: seconds are "sn"; percent goes before the number ("%{percent}").
- **Plurals**: Turkish uses `one` and `other`, and a noun after a number stays singular
  ("3 klip"), so both forms usually read the same.

## Words kept apart

- Reverse (playback) is "Tersten Oynat"; "Ters Çevir" is only the mask's Invert.
- Lossless export's Copy mode is "Akış Kopyası", never Edit's "Kopyala".
- Lift is "Boşluk Bırakarak Kaldır" and Extract "Boşluğu Kapatarak Çıkar", so neither reads as
  Remove ("Kaldır").
- Where English "frame" means the picture's canvas (Project Settings), Turkish says
  "görüntü alanı"; "kare" is always one picture.
