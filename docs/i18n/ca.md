# Glossari català

The Catalan translation of the [glossary](glossary.md): one word per term, used everywhere the
term appears in the `ca.toml` catalogs. See [Translating jvid](../translating.md).

Neither DaVinci Resolve nor Premiere Pro ships a Catalan interface, so the choices follow
Shotcut's Catalan translation (checked against its `shotcut_ca.ts`) and Kdenlive's, with
Softcatalà's conventions deciding where those read oddly. Terms marked *unconfirmed* have no
settled editing-software usage and want a native editor's eye first.

| Term | Català | Shotcut | Why |
| --- | --- | --- | --- |
| Project | projecte | projecte | |
| Media | mitjans | fitxers multimèdia | The panel is Mitjans; one file is a mitjà (Mitjà fora de línia). *Unconfirmed.* |
| Bin | safata | safata | Kdenlive's Safata del projecte agrees. The media bin is the safata de mitjans. |
| Clip | clip (m.) | clip | Plural clips. |
| Track | pista | pista | |
| Timeline | línia de temps | línia de temps | |
| Playhead | capçal de reproducció | capçal de reproducció | Always in full. |
| Source | Origen | Origen | The panel title. |
| Program | Programa | | The panel title. |
| Viewer | visor | reproductor | |
| Inspector | Inspector | Propietats | jvid's panel name, a Catalan word. |
| Effect | efecte | filtre | |
| Transition | transició | transició | |
| Keyframe | fotograma clau | fotograma clau | Plural fotogrames clau. |
| Marker | marcador | marcador | |
| In point | punt d'entrada | punt d'entrada | Short labels say Entrada. |
| Out point | punt de sortida | punt de sortida | Short labels say Sortida; the export file section is Destinació so the two never meet. |
| Edit point | punt d'edició | | |
| Range | interval | interval, rang | |
| Trim | ajustar | ajusta | Retallar is taken by the clipboard's Retalla (Cut). |
| Ripple | amb propagació | propaga | Suprimeix amb propagació, Ajusta amb propagació. |
| Roll | desplaçament del tall | | Docs only. *Unconfirmed.* |
| Slip | lliscament | | Docs only. *Unconfirmed.* |
| Split | dividir | divideix | Divideix al capçal de reproducció, as Shotcut. |
| Nest | niar | | Seqüència niada, as KDE says for nested items. *Unconfirmed.* |
| Snapping | ajust magnètic | imant | Shotcut's Imant names the icon, not the feature. *Unconfirmed.* |
| Link | enllaçar | enllaça | Linked Selection is Selecció enllaçada. |
| Lock | bloquejar | bloqueja | |
| Mute | silenciar | silencia | |
| Solo | solo | | |
| Scrubbing | exploració | explora | Shotcut's Explora en arrossegar. *Unconfirmed.* |
| Shuttle | llançadora | | The broadcast term (jog/llançadora). |
| Timecode | codi de temps | codi de temps | |
| Frame rate | freqüència de fotogrames | fotogrames per segon | Units say fps. |
| Resolution | resolució | resolució | |
| Aspect ratio | relació d'aspecte | relació d'aspecte | |
| Proxy | clip intermediari | servidor intermediari | Kdenlive's term; Shotcut's is the network proxy. Plural intermediaris. |
| Render | renderitzar, renderització | | |
| Export | exportar, exportació | exporta | |
| Render Queue | cua de renderització | cua | |
| Preset | predefinició (f.) | predefinició | |
| Codec | còdec | còdec | Names stay as written. |
| Bitrate | taxa de bits | taxa de bits | |
| Title | títol | títol | A lower third is a rètol inferior. |
| Subtitles | subtítols | subtítols | |
| Waveform | forma d'ona | forma d'ona | |
| Mixer | mesclador | mesclador | |
| Voiceover | veu en off | veu en off | |
| Undo / Redo | Desfés / Refés | Desfés / Refés | As in Windows and macOS. |

## Conventions

- **Capitalization**: sentence case everywhere, labels included (Desa com a, Cua de
  renderització). Panel names (Mitjans, Origen, Programa, Inspector) keep their capital when
  named in running text.
- **Commands** (menus, buttons, keyboard actions) use the second-person singular imperative,
  as Windows, macOS, Softcatalà and Shotcut do: Desa, Obre, Afegeix un marcador. Toggles and
  settings are nouns (Ajust magnètic, Selecció enllaçada). **Messages** address the user as vós:
  Seleccioneu un clip…, Voleu desar els canvis…?
- **Quotation marks**: «angulars» around names (Voleu eliminar «{name}»?). Apostrophes are
  straight, as in the other catalogs.
- **Numbers and units**: a space before % and units ({percent} %, {value} MB, {seconds} s).
  Shortcut notation (Ctrl+Shift+Z, Esc, Backspace), codec and format names stay as written.
- **Dates**: day, then month with its preposition, as CLDR's Catalan does: 12 de gen. 2026,
  3 d'abr.
- **Plurals**: `one` is 1; `many` (exact millions) takes de before a noun, elided before a vowel
  (1000000 de clips, 1000000 d'hores).
