# Glossaire français

The French translation of the [glossary](glossary.md): one word per term, used everywhere the
term appears in the `fr.toml` catalogs. Each choice follows what French-speaking editors see in
the localized DaVinci Resolve and Premiere Pro, with Final Cut Pro deciding where those two
disagree. See [Translating jvid](../translating.md).

| Term | Français | Resolve | Premiere Pro | Why |
| --- | --- | --- | --- | --- |
| Project | projet | projet | projet | |
| Media | médias | médias | médias | |
| Bin | chutier | chutier | chutier | The panel itself is titled Médias, like jvid's English Media. |
| Clip | plan | plan | élément | Resolve and Final Cut Pro both say plan; Premiere's élément also means any item. |
| Track | piste | piste | piste | |
| Timeline | timeline (f.) | timeline | Montage (panel), séquence | Resolve and Final Cut Pro keep timeline; Premiere's Montage would clash with the act of editing. |
| Playhead | tête de lecture | tête de lecture | tête de lecture | |
| Source | Source | viewer source | Moniteur source | The panel title alone, as in jvid's English. |
| Program | Programme | viewer de la timeline | Moniteur de programme | The panel title alone. |
| Viewer | visualiseur | viewer | moniteur | Resolve keeps English; Final Cut Pro's visualiseur is the French word. |
| Inspector | inspecteur | inspecteur | Options d'effet | Resolve and Final Cut Pro agree. |
| Effect | effet | effet | effet | |
| Transition | transition | transition | transition | |
| Keyframe | image clé | image clé | image clé | Plural images clés. |
| Marker | marqueur | marqueur | repère / marque | Resolve and Final Cut Pro agree. |
| In point | point d'entrée | point d'entrée | point d'entrée | Short labels say Entrée. |
| Out point | point de sortie | point de sortie | point de sortie | Short labels say Sortie. |
| Range | plage | plage | plage | |
| Trim | rogner | trim, raccord fin | raccord | Resolve keeps English and Final Cut Pro says élagage; rogner is the verb both Resolve's French guide and French Premiere tutorials use for it. |
| Ripple | propagation (raccorder) | Ripple | propagation, raccorder | Resolve keeps English (Supprimer et Ripple); Premiere's Supprimer et raccorder and Modification par propagation are French. |
| Roll | modification compensée | Roll | modification compensée | Premiere's French term; Resolve and Final Cut Pro keep Roll. |
| Slip | glissement | coulissement | glissement | Premiere and Final Cut Pro agree. |
| Split | scinder | scinder | ajouter un montage | Resolve's verb, the clearer of the two. |
| Snapping | magnétisme | magnétisme | magnétisme | |
| Link | lier | lier | lier | Linked Selection is Sélection liée in both. |
| Lock | verrouiller | verrouiller | verrouiller | |
| Mute | couper le son | couper le son | désactiver le son | Resolve's shorter phrase. |
| Solo | solo | solo | solo | |
| Scrubbing | défilement | survol, scrubbing | défilement audio | Resolve's survol is for skimming in the bin; Premiere's term is the playhead one. |
| Shuttle | navette | touches JKL | navette | The broadcast term (jog/navette); Resolve's guide only names the keys. |
| Timecode | timecode | timecode | code temporel | Resolve and Final Cut Pro keep timecode. |
| Frame rate | fréquence d'images | fréquence d'images | fréquence d'images | Units say i/s. |
| Resolution | résolution | résolution | résolution | |
| Aspect ratio | format d'image | format d'image | rapport L/H | Resolve's term reads without abbreviation. |
| Proxy | proxy | proxy | proxy | Plural proxys. |
| Render | rendu | rendu | rendu | |
| Export | exporter, exportation | exporter | exporter | |
| Render Queue | file d'attente de rendu | file d'attente de rendu | file d'attente (Media Encoder) | |
| Preset | préréglage | préréglage | paramètre prédéfini | Resolve's word is shorter, which matters in dropdowns. |
| Codec | codec | codec | codec | Names stay as written. |
| Bitrate | débit | débit binaire | débit | Short form; débit binaire where it could be misread. |
| Title | titre | titre | titre | |
| Subtitles | sous-titres | sous-titres | sous-titres | |
| Waveform | forme d'onde | forme d'onde | forme d'onde | |
| Mixer | mixeur | mixeur | mixage des pistes audio | Resolve's Fairlight term. |
| Voiceover | voix off | voix off | voix off | |
| Undo / Redo | annuler / rétablir | annuler / rétablir | annuler / rétablir | The Windows and macOS menu words. |

## Conventions

- **Capitalization**: sentence case everywhere, labels included (Nouveau projet, Tout
  sélectionner), as French Windows, macOS, Resolve and Premiere do. Proper nouns and the panel
  names Source and Programme keep their capital in running text.
- **Commands** (menus, buttons, keyboard actions) use the infinitive: Enregistrer sous…,
  Exporter l'image, Ajouter un marqueur. **Instructions** to the user use the imperative with
  vous: Sélectionnez un plan…, Déposez des médias ici….
- **Punctuation**: a no-break space (U+00A0) before `:` `;` `?` `!` and inside « guillemets »;
  the bundled Inter subset has no narrow no-break space (U+202F), so the wide one stands in.
- **Numbers and units**: a space before % and units ({percent} %, {value} Mo); file sizes in
  Ko, Mo and Go; frame rates in i/s. Shortcut notation (Ctrl+Shift+Z), codec and format names
  stay as written.
- **Dates**: day before month, lower-case abbreviated months (12 janv. 2026).
- **Plurals**: French's `one` covers 0 and 1, so "0 plan" is singular. The `many` form (exact
  millions) repeats `other`: written in digits, "1000000 plans" needs no "de".

## Sources checked

- Blackmagic Design, *Guide complet de DaVinci Resolve 15* (French edition, translated by
  Blackmagic Design): <https://documents.blackmagicdesign.com/fr/UserManuals/DaVinci-Resolve-15-Definitive-Guide.pdf>.
  Source of chutier, plan, tête de lecture, marqueur, inspecteur, images clés, scinder un plan,
  Supprimer et Ripple, raccord fin Roll, raccord par coulissement, magnétisme, outil Rasoir,
  préréglage, débit binaire, fréquence d'images, format d'image, mixeur, voix off, fondu
  enchaîné, viewer source.
- Apple, *Guide d'utilisation de Final Cut Pro* (French): <https://support.apple.com/fr-fr/guide/final-cut-pro/ver9f7888dc3/mac>.
  Plan, tête de lecture, timeline, magnétisme, Lame de rasoir, images clés, marqueurs,
  visualiseur, inspecteur, Élagage, montages par glissement.
- French Premiere Pro training material quoting the French interface (Adobe's own French help
  pages refused automated requests): Elephorm on Supprimer et raccorder, Magnétisme, Outil
  Modification compensée and Outil Cutter
  (<https://www.elephorm.com/tuto-premiere-pro-cc/magnetisme>,
  <https://www.elephorm.com/tuto-premiere-pro-cc/outil-modification-compensee>); Éditions ENI,
  *Premiere Pro 2025* (Déplacement de la coupe, aussi appelé modification compensée); Leptidigital's
  2026 shortcut list (Moniteur de programme, Options d'effet, Supprimer et raccorder, Chutier).
