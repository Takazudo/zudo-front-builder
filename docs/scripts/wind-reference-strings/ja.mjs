export const strings = {
  index: {
    title: "ユーティリティリファレンス",
    description:
      "有限の zudo-wind v1 ユーティリティカタログを、ユーティリティ群と宣言から参照します。",
    intro:
      "実現したい用途からユーティリティ群を選びます。レイアウトと余白、文字、背景や境界線、動き、操作の順に探せます。各ページには対応する値、宣言、詳細なカタログ情報も残しています。",
    specVersionLabel: "カタログの仕様バージョン",
    specRevisionLabel: "カタログの仕様リビジョン",
    familyColumn: "ユーティリティ群",
    entryCountColumn: "カタログ項目数",
  },
  labels: {
    field: "フィールド",
    value: "値",
    catalogIdentifier: "カタログ識別子",
    classPattern: "クラスパターン",
    grammar: "文法",
    acceptedValues: "受け入れる値",
    tokenCategories: "トークンカテゴリ",
    negativePolicy: "負の値の規則",
    conflictGroup: "競合グループと順位",
    orderRank: "並び順の順位",
    selectorShape: "セレクタの形",
    specVersion: "仕様バージョン",
    emitterProperties: "出力するプロパティ",
    noAcceptedValues: "この項目には独立した値の定義がありません。",
    noTokenCategories: "トークンカテゴリなし",
    noDeclarations: "宣言の定義はありません",
    noExamples: "この項目にはカタログの例がありません。",
  },
  headings: {
    acceptedValues: "受け入れる値",
    declarationTemplates: "宣言テンプレート",
    examples: "カタログの例",
    relatedGuides: "関連ガイド",
  },
  columns: {
    kind: "種類",
    suffix: "接尾辞",
    emittedValue: "出力される値",
    property: "プロパティ",
    valueKind: "値の種類",
    fixedValue: "固定値",
    candidate: "クラスの例",
    expectedDeclarations: "期待される宣言",
  },
  links: {
    utilityGrammar: "ユーティリティの文法",
    variants: "バリアント",
  },
  reader: {
    lookup: "クラスと CSS の早見表",
    setup: "例の設定",
    workedExamples: "使い方の例",
    customValues: "任意値と名前付きの値",
    technical: "カタログの詳細",
    config: "この例の設定",
    scaffold: "デモ用の自作 CSS",
    diagnostics: "期待される診断",
    taskColumn: "用途",
    browse: "用途から選ぶ",
  },
  families: {
    svg: {
      title: "SVG の塗り",
      description: "SVG の塗りと線を currentColor または none に設定します。",
    },
    display: {
      title: "表示",
      description: "要素の非表示、インライン、ブロック、フレックス、グリッドを選びます。",
    },
    position: {
      title: "配置方式",
      description: "包含ブロック内での要素の配置方式を設定します。",
    },
    inset: {
      title: "位置オフセット",
      description: "余白値、分数、auto、full を使い、物理的な辺のオフセットを設定します。",
    },
    "flex-container": {
      title: "フレックスコンテナ",
      description: "フレックスコンテナの主軸方向と折り返しを設定します。",
    },
    "flex-item": {
      title: "フレックス項目",
      description: "項目の伸長、収縮、簡略指定のサイズ動作を設定します。",
    },
    grid: {
      title: "グリッド",
      description: "グリッドトラックを定義し、行・列の範囲と線に沿って項目を配置します。",
    },
    alignment: {
      title: "整列",
      description: "対応する軸に沿って項目、個別の項目、グリッド内容を整列します。",
    },
    sizing: {
      title: "サイズ",
      description: "余白、サイズトークン、分数、キーワードを使い、幅と高さの制約を設定します。",
    },
    padding: {
      title: "内側の余白",
      description: "全辺、軸、個別の辺に物理的な内側の余白を設定します。",
    },
    margin: {
      title: "外側の余白",
      description: "対応する負の値と自動余白を含め、物理的な外側の余白を設定します。",
    },
    gap: {
      title: "間隔",
      description: "レイアウトコンテナ内の項目間の行・列の間隔を設定します。",
    },
    "child-space": {
      title: "子要素間の余白",
      description: "直接の子セレクタを使い、後続の表示対象の兄弟要素間に余白を追加します。",
    },
    overflow: {
      title: "オーバーフロー",
      description: "要素または軸の内容のオーバーフローとオーバースクロールを制御します。",
    },
    "z-index": {
      title: "重なり順",
      description: "範囲付き整数、auto、設定済みトークンで重なり順を設定します。",
    },
    "font-family": {
      title: "フォントファミリー",
      description: "設定済みフォントファミリーを選ぶか、任意の font-family 値を指定します。",
    },
    "font-weight": {
      title: "フォントウェイト",
      description: "設定済みフォントウェイトを選ぶか、任意のウェイト値を指定します。",
    },
    "font-size": {
      title: "文字サイズ",
      description: "設定済みトークンまたは任意の CSS font-size 値から文字サイズを設定します。",
    },
    "line-height": {
      title: "行の高さ",
      description:
        "設定済みトークンまたは任意の CSS 値から行の高さを設定します。none トークンを設定していない場合、leading-none は 1 になります。",
    },
    tracking: {
      title: "字間",
      description: "設定済みトークンまたは任意の値で字間を設定します。負の値も使えます。",
    },
    "text-layout": {
      title: "テキスト配置",
      description: "文字の整列、空白、折り返し、省略表示を設定します。",
    },
    "text-style": {
      title: "テキスト装飾",
      description: "文字の装飾、大小文字、フォントスタイル、数字の形、平滑化を設定します。",
    },
    color: {
      title: "文字色",
      description: "設定された色の値または検証済みの任意の色から文字色を設定します。",
    },
    background: {
      title: "背景色",
      description: "設定された値または検証済みの任意の色から背景色を設定します。",
    },
    "border-width": {
      title: "ボーダー幅",
      description: "物理的なボーダー幅をピクセルで設定し、実線のボーダースタイルにします。",
    },
    "border-color": {
      title: "ボーダー色",
      description: "設定された値または検証済みの任意の色から各辺のボーダー色を設定します。",
    },
    "border-style": {
      title: "ボーダースタイル",
      description: "対応する線種またはボーダーの結合動作を選びます。",
    },
    radius: {
      title: "角丸",
      description: "設定された角丸の値を、全角、辺の角、個別の角に適用します。",
    },
    "divide-width": {
      title: "区切り線の幅",
      description: "後続の表示対象の兄弟要素に、軸に沿った実線のボーダー幅を設定します。",
    },
    "divide-color": {
      title: "区切り線の色",
      description: "グループ内で後続の表示対象の兄弟要素のボーダー色を設定します。",
    },
    "outline-width": {
      title: "アウトラインの幅とオフセット",
      description:
        "長さでアウトラインの幅またはオフセットを設定します。負の値はオフセットのみ使えます。",
    },
    "outline-color": {
      title: "アウトライン色",
      description: "設定された値または検証済みの任意の色からアウトライン色を設定します。",
    },
    "outline-style": {
      title: "アウトラインスタイル",
      description: "none を含む、対応するアウトラインの線種を選びます。",
    },
    shadow: {
      title: "シャドウ",
      description: "設定済みのボックスシャドウを適用するか、シャドウを取り除きます。",
    },
    opacity: {
      title: "不透明度",
      description: "要素の不透明度を整数の割合または検証済みの任意の値で設定します。",
    },
    transition: {
      title: "トランジション",
      description: "トランジション対象のプロパティ群と組み込みのタイミング初期値を選びます。",
    },
    duration: {
      title: "トランジション時間",
      description: "範囲付きのミリ秒値または任意の時間でトランジション時間を設定します。",
    },
    easing: {
      title: "イージング",
      description: "設定済みイージングトークンまたは任意のタイミング関数を指定します。",
    },
    translate: {
      title: "移動",
      description: "余白、full、分数、負の値を使い、軸に沿って要素を移動します。",
    },
    rotate: {
      title: "回転",
      description: "対応する角度または任意の角度値で要素を回転します。",
    },
    interaction: {
      title: "操作",
      description: "カーソル、ポインター、選択、リサイズ、アクセント色の動作を設定します。",
    },
    list: {
      title: "リストスタイル",
      description:
        "リストマーカーと、マーカーを内容ボックスの内側・外側のどちらに置くかを設定します。",
    },
    aspect: {
      title: "アスペクト比",
      description: "自動、名前付き、または任意のアスペクト比を設定します。",
    },
    "scroll-margin": {
      title: "スクロール余白",
      description: "負の値も含む余白値で、物理的なスクロール余白を設定します。",
    },
    miscellaneous: {
      title: "その他",
      description:
        "縦方向の整列、ボックスサイズ、オブジェクトの収まり、スクリーンリーダー専用の切り抜きを設定します。",
    },
    "text-decoration-color": {
      title: "テキスト装飾",
      description: "カタログ値を使ってテキスト装飾の色、太さ、線種を設定します。",
    },
    "text-underline-offset": {
      title: "下線のオフセット",
      description: "下線のオフセットを整数のピクセル値、または 0 以上の任意の長さで設定します。",
    },
    visibility: {
      title: "表示状態",
      description: "レイアウト上の位置を保ったまま、要素を表示または非表示にします。",
    },
  },
};

export default strings;
