# Glossary: Portuguese (Brazil)

The [glossary](glossary.md)'s terms as jvid's Brazilian Portuguese catalogs use them. Each one
follows what a Brazilian editor sees in the localized Premiere Pro (Adobe's pt-BR interface and
help) or DaVinci Resolve (Blackmagic's pt-BR interface and site); where the two disagree, the
note says which was picked and why. Use the one word everywhere the term appears; see
[Translating jvid](../translating.md).

## Conventions

- **Capitalization**: sentence case for every label, menu item, button and tab ("Novo projeto",
  "Adicionar à fila"), as Windows, macOS and Premiere's pt-BR interface do. Inside a
  sentence, terms stay lower case ("Mostrar no compartimento de mídia").
- **Commands** (menu items, buttons, keyboard actions) use the infinitive: "Salvar",
  "Desfazer", "Exportar mídia", "Adicionar marcador", as Windows and macOS menus do in pt-BR.
- **Instructions** addressed to the user use the imperative of *você*: "Selecione um clipe…",
  "Arraste clipes para cá…", "Defina a entrada ou a saída…".
- **Quotation marks**: “curly double” quotes, as in English.
- **Plurals**: CLDR's `one` covers 0 and 1 in Portuguese, so `one` forms keep `{count}` rather
  than spelling out "1". `many` (exact millions) takes "de" before the noun: "1000000 de clipes".
- **Numbers, durations and timecodes** come from code and keep the decimal point.

## Terms

| Term | pt-BR | Premiere Pro | DaVinci Resolve | Note |
| --- | --- | --- | --- | --- |
| Project | projeto | Projeto | Projeto | |
| Media | mídia | Mídia | Mídia ("Pool de mídia") | |
| Bin | compartimento | Compartimento | Bin (inside the "Pool de mídia") | Premiere's word: a translated noun reads better in sentences than the English "bin", and Premiere is the larger pt-BR audience. jvid's bin panel is titled "Mídia". |
| Clip | clipe | Clipe | Clipe | |
| Track | faixa | Faixa ("Faixa de vídeo 1") | Trilha ("trilha de vídeo") | Premiere's "faixa"; Resolve's own pt-BR pages mix "trilha" and "faixa". |
| Timeline | linha do tempo | Linha do tempo | Linha de tempo | Premiere's form is the common Brazilian usage. |
| Playhead | indicador de reprodução | Indicador de reprodução | Cursor de reprodução | Premiere's. |
| Source | origem | Monitor de origem | Visualizador de origem | Both use "origem". The panel tab says just "Origem". |
| Program | programa | Monitor de programa | Visualizador da linha do tempo | Premiere's; jvid's panel is a named program monitor. |
| Viewer | visualizador | Monitor | Visualizador | Resolve's: jvid's two picture panels are viewers, not hardware monitors. |
| Inspector | inspetor | (Controles de efeito) | Inspetor | Resolve's; Premiere has no panel by this name. |
| Effect | efeito | Efeito | Efeito | |
| Transition | transição | Transição | Transição | |
| Keyframe | quadro-chave (pl. quadros-chave) | Quadro-chave | Quadro-chave | |
| Marker | marcador | Marcador | Marcador | |
| In point | ponto de entrada; "Entrada" on short labels | Ponto de entrada ("Marcar entrada") | Ponto de entrada | |
| Out point | ponto de saída; "Saída" on short labels | Ponto de saída ("Marcar saída") | Ponto de saída | |
| Range | intervalo | Intervalo | Intervalo | |
| Trim | aparar | Aparar | Aparar | |
| Ripple | ondulação ("excluir com ondulação") | Edição de ondulação, Exclusão de ondulação | Ripple (kept in English) | Premiere's; Resolve leaves it untranslated. |
| Roll | rolagem | Edição de rolagem | Roll | Premiere's. Not in jvid's strings yet. |
| Slip | deslizamento | Deslizamento ("edições de deslizamento") | Slip | Adobe's help says "deslizamento"; some shortcut lists call Premiere's slip tool "Escorregar". Not in jvid's strings yet. |
| Split | dividir | Dividir | Dividir | "Dividir no indicador de reprodução". |
| Snapping | encaixe | Ajuste / snap (label unconfirmed) | Encaixe | Resolve's; Premiere's pt-BR label couldn't be confirmed. |
| Link | vincular / desvincular | Vincular, Seleção vinculada | Vincular, Seleção vinculada | |
| Lock | bloquear | Bloquear faixa | Bloquear trilha | |
| Mute | silenciar | Silenciar faixa | Mudo | Premiere's verb, matching the other track toggles. |
| Solo | solo | Solo | Solo | |
| Scrubbing | arrastar (scrubbing) | Scrubbing de áudio | Scrubbing de áudio | Both keep the English noun; jvid says what happens instead: "Áudio ao arrastar". |
| Shuttle | shuttle | Shuttle | Shuttle | Kept in English, as both do. |
| Timecode | código de tempo | Código de tempo | Timecode | Premiere's. Not in jvid's strings yet. |
| Frame rate | taxa de quadros | Taxa de quadros | Taxa de quadros | |
| Resolution | resolução | Resolução | Resolução | |
| Aspect ratio | proporção | Proporção | Proporção | |
| Proxy | proxy (pl. proxies) | Proxy | Proxy | |
| Render | renderizar | Renderizar | Renderizar | |
| Export | exportar | Exportar | Exportar | |
| Render Queue | fila de renderização | Fila (Media Encoder) | Fila de renderização | Resolve's. |
| Preset | predefinição | Predefinição | Predefinição | |
| Codec | codec | Codec | Codec | Codec names stay as written. |
| Bitrate | taxa de bits | Taxa de bits | Taxa de bits | |
| Title | título | Título | Título | |
| Subtitles | legendas | Legendas | Legendas | |
| Waveform | forma de onda | Forma de onda | Forma de onda | |
| Mixer | mixer | Mixer de faixas de áudio | Mixer | |
| Voiceover | narração | Narração ("Gravação de narração") | Locução | Premiere's. |
| Undo / Redo | desfazer / refazer | Desfazer / Refazer | Desfazer / Refazer | |

Other recurring words:

- **Cut** (clipboard) is "Recortar", as Windows and macOS say; **Crop** is "Cortar", Premiere's
  name for its Crop effect; a cut point between two clips is a "corte".
- **Overwrite** is "Sobrescrever" (Resolve), not Premiere's "Substituir", which pt-BR apps also
  use for Replace.
- **Razor** is "Lâmina" ("Ferramenta Lâmina"), as in Premiere.
- **Crossfade** is "Fusão cruzada"; **Dip to Black/White** is "Fade para preto/branco", the
  wording Brazilian editors use; Premiere's own transition names weren't confirmed.
- **Bypass** (an effect switched off, settings kept) is "Desligar", paired with "Ligar".
- **Threshold** is "Limiar" in both Detect Silence and the compressor.

## Sources checked

- Adobe Premiere Pro help, pt-BR: [Definir pontos de entrada e de saída no Monitor de
  origem](https://helpx.adobe.com/br/premiere-pro/using/set-in-out-points-source-monitor.html),
  [Reprodução e visualização de ativos](https://helpx.adobe.com/br/premiere-pro/using/playing-assets.html)
  ("indicador de reprodução", "Marcar entrada", "Monitor de programa"),
  [Aparar clipes](https://helpx.adobe.com/br/premiere-pro/using/trimming-clips.html)
  ("edições de deslizamento"),
  [Correção de sequências de origem](https://helpx.adobe.com/br/premiere-pro/using/source-patching-track-targeting.html)
  (track lock and targeting), and the pt-BR help navigation ("compartimentos").
- Premiere tool names and shortcuts in pt-BR: [Domestika, barra de
  ferramentas](https://www.domestika.org/pt/blog/5050-tutorial-adobe-premiere-pro-introducao-a-barra-de-ferramentas)
  ("Edição de ondulação", "Edição de rolagem"),
  [Oficina da Net](https://www.oficinadanet.com.br/tecnologia/26228-os-melhores-atalhos-do-adobe-premiere)
  ("Exclusão de ondulação", "Deslizar", "Escorregar").
- Blackmagic's pt-BR DaVinci Resolve pages: [Edição](https://www.blackmagicdesign.com/br/products/davinciresolve/edit)
  ("trilha de vídeo", "cursor de reprodução", "linha de tempo", "Inserir por Cima"),
  [Corte](https://blackmagicdesign.com/pt/products/davinciresolve/cut) ("editor de
  quadros-chave", "locução"), [Mídias](https://www.blackmagicdesign.com/br/products/davinciresolve/media)
  ("Inspetor", "visualizador de origem").
- Resolve shortcut lists in pt-BR: [videohero.com.br](https://videohero.com.br/atalhos-davinci)
  ("encaixe", "aparar").
- Adobe Media Encoder in pt-BR: [Tonka3D](https://www.tonka3d.com.br/blog/adobe-media-encoder/)
  ("fila", "predefinição", "taxa de bits").

Not confirmed from a source: Premiere's pt-BR label for the snapping toggle, and Final Cut Pro's
pt-BR terms (Apple's pt-BR user guide is still published in English). These are worth a check
by a native speaker with the localized apps open.
