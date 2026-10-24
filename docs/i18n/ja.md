# Japanese glossary (日本語)

The [glossary](glossary.md)'s terms as jvid's Japanese catalogs write them. Each follows the
Japanese DaVinci Resolve or Premiere Pro interface; where those two disagree, the note says which
one jvid follows and why (Final Cut Pro is the tiebreaker). Use the one word below every time the
term appears; see [Translating jvid](../translating.md).

| Term | 日本語 | Resolve | Premiere Pro | Note |
| --- | --- | --- | --- | --- |
| Project | プロジェクト | プロジェクト | プロジェクト | |
| Media | メディア | メディア | メディア | |
| Bin | ビン | ビン | ビン | jvid's Media panel tab is メディア; the Media Bin is メディアビン. |
| Clip | クリップ | クリップ | クリップ | |
| Track | トラック | トラック | トラック | |
| Timeline | タイムライン | タイムライン | タイムライン | Premiere's container is シーケンス; jvid keeps シーケンス for its nested sequences, as Premiere does. |
| Playhead | 再生ヘッド | 再生ヘッド | 再生ヘッド | |
| Source | ソース | ソースビューア | ソースモニター | The panel title is just ソース, as English's is Source. |
| Program | プログラム | タイムラインビューア | プログラムモニター | Premiere's word: jvid's English says Program, and Resolve's "timeline viewer" would read as a second Timeline. |
| Viewer | ビューア | ビューア | モニター | Resolve and Final Cut both say ビューア (no long-vowel mark, Apple and Blackmagic style). |
| Inspector | インスペクタ | インスペクタ | エフェクトコントロール | Premiere has no Inspector; Resolve and Final Cut write インスペクタ without ー. |
| Effect | エフェクト | エフェクト | エフェクト | |
| Transition | トランジション | トランジション | トランジション | |
| Keyframe | キーフレーム | キーフレーム | キーフレーム | |
| Marker | マーカー | マーカー | マーカー | |
| In point | イン点 | イン点 | イン点 | Commands shorten it to イン (インをマーク, インへ移動), as Premiere does. |
| Out point | アウト点 | アウト点 | アウト点 | Likewise アウト in commands. |
| Range | 範囲 | 範囲 | 範囲 | |
| Trim | トリム | トリム | トリミング | Resolve's word, shorter; Premiere's リップルトリミング becomes リップルトリム. |
| Ripple | リップル | リップル | リップル | リップル削除 in both. |
| Roll | ロール | ロール | ローリング | Resolve's word; Premiere's ローリング編集 is the tool name. |
| Slip | スリップ | スリップ | スリップ | |
| Split | 分割 | クリップを分割 / ブレード | 編集点を追加 | jvid's English says Split, not Add Edit, so the plain word 分割 (Resolve's menu). |
| Snapping | スナップ | スナップ | スナップ | |
| Link | リンク | リンク | リンク | Linked Selection is リンク選択, Resolve's name. |
| Lock | ロック | ロック | ロック | |
| Mute | ミュート | ミュート | ミュート | |
| Solo | ソロ | ソロ | ソロ | |
| Scrubbing | スクラブ | スクラブ | スクラブ | Audio Scrubbing: オーディオスクラブ. |
| Shuttle | シャトル | シャトル | シャトル | |
| Timecode | タイムコード | タイムコード | タイムコード | |
| Frame rate | フレームレート | フレームレート | フレームレート | |
| Resolution | 解像度 | 解像度 | 解像度 | |
| Aspect ratio | アスペクト比 | アスペクト比 | 縦横比 | Resolve's; Premiere's 縦横比 is mostly seen as ピクセル縦横比. アスペクト is the short label. |
| Proxy | プロキシ | プロキシ | プロキシ | |
| Render | レンダリング | レンダー | レンダリング | The act is Premiere's レンダリング, the everyday word; the queue keeps Resolve's compound below. |
| Export | 書き出し | 書き出し / デリバー | 書き出し | Buttons and menus say 書き出し; a sentence uses 書き出す. |
| Render Queue | レンダーキュー | レンダーキュー | (Media Encoder: キュー) | Resolve and After Effects both name it レンダーキュー. |
| Preset | プリセット | プリセット | プリセット | |
| Codec | コーデック | コーデック | コーデック | Codec names stay as written. |
| Bitrate | ビットレート | ビットレート | ビットレート | |
| Title | タイトル | タイトル | タイトル | |
| Subtitles | 字幕 | 字幕 | キャプション | Resolve's; jvid's English says Subtitles and imports .srt, which Japanese users call 字幕. |
| Waveform | 波形 | 波形 | 波形 | The scope and the audio drawing are both 波形. |
| Mixer | ミキサー | ミキサー | オーディオトラックミキサー | |
| Voiceover | ボイスオーバー | ボイスオーバー | ボイスオーバー | |
| Undo / Redo | 元に戻す / やり直し | 元に戻す / やり直し | 取り消し / やり直し | Resolve's pair is also Windows' standard Edit menu. |

## Conventions

- **Buttons and menu items** are nouns or noun phrases, as on Windows and macOS: 保存, 削除,
  書き出し, 新規プロジェクト, 名前を付けて保存…, キャンセル. Commands that act on something say
  what: インをマーク, マーカーを追加, 再生ヘッドで分割. No polite -ます form on a button.
- **Messages** use the polite form (〜できません, 〜しますか？); short status lines and toasts may
  end in a noun (書き出し完了, 待機中).
- **Error details** shown after "Couldn't …:" are plain-form fragments (読み取れるメディアファイルではありません
  is the exception where a full sentence reads better), joined with a full-width colon (：).
- **Punctuation**: full-width 、。？：「」 in Japanese text; English and ASCII stay half-width.
  A name the user gave is quoted 「{name}」. The ellipsis … stays as the English has it.
- **Spaces**: a half-width space separates a `{placeholder}` or Latin word from Japanese text
  when it would otherwise run together (`{count} 個のクリップ`, `{path} を開けませんでした`).
- **Counters**: clips 個, exports 件, regions 箇所, sources 個; Japanese has only the `other`
  plural form.
- **Dates**: `{month}` is 1月…12月, so 今年 is `{month}{day}日` and other years
  `{year}年{month}{day}日`.

## Sources checked

- Blackmagic Design, DaVinci Resolve product pages in Japanese (Cut and Edit pages):
  https://www.blackmagicdesign.com/jp/products/davinciresolve/edit
- DaVinci Resolve 12 Reference Manual, Japanese edition (デリバーページ, レンダーキュー):
  https://documents.blackmagicdesign.com/jp/UserManuals/DaVinci_Resolve_12_Reference_Manual.pdf
- Resolve Japanese menu names (元に戻す, やり直し, リップル削除, ブレード編集モード, リンク選択,
  スナップ): https://nextist.net/davinci-resolve-shortcut-key-list/ and
  https://ytktfeelfree.com/tool/software/davinci_resolve/shortcut_for_davinci_menu1_/6245/
- Resolve イン点 / アウト点, 再生ヘッド, タイムラインビューアー: https://vip-jikkyo.net/?p=20242
- Adobe Premiere Pro help in Japanese (再生ヘッド, ソースモニター, プログラムモニター,
  エフェクトコントロール): https://helpx.adobe.com/jp/premiere/desktop/render-and-export/render-sequences-for-playback/play-back-in-a-continuous-loop.html
- Premiere Japanese command names (前の編集点を再生ヘッドまでリップルトリミング, リップル削除):
  https://soho-biyori.com/premiere-pro-cut-shortcut and https://ks-llc.co.jp/media/premiere-pro-cut-ripple-delete/
- Premiere オーディオトラックミキサー and ボイスオーバー録音:
  https://helpx.adobe.com/jp/premiere/desktop/add-audio-effects/apply-audio-effects/copy-and-move-effects-in-audio-track-mixer.html
