import { posix } from "node:path";
import { vi } from "vitest";

export class Disposable {
  private callOnDispose: (() => void) | undefined;

  constructor(callOnDispose?: () => void) {
    this.callOnDispose = callOnDispose;
  }

  dispose(): void {
    const call = this.callOnDispose;
    this.callOnDispose = undefined;
    call?.();
  }

  static from(...disposables: { dispose(): void }[]): Disposable {
    return new Disposable(() => {
      for (const disposable of disposables) disposable.dispose();
    });
  }
}

export class EventEmitter<T> {
  private listeners = new Set<(event: T) => void>();

  readonly event: (
    listener: (event: T) => void,
    thisArgs?: unknown,
    disposables?: Disposable[],
  ) => Disposable;

  constructor() {
    this.event = (listener, thisArgs, disposables) => {
      const bound = thisArgs ? listener.bind(thisArgs) : listener;
      this.listeners.add(bound);
      const disposable = new Disposable(() => {
        this.listeners.delete(bound);
      });
      disposables?.push(disposable);
      return disposable;
    };
  }

  fire(event: T): void {
    for (const listener of [...this.listeners]) listener(event);
  }

  dispose(): void {
    this.listeners.clear();
  }
}

export enum StatusBarAlignment {
  Left = 1,
  Right = 2,
}

export enum TreeItemCollapsibleState {
  None = 0,
  Collapsed = 1,
  Expanded = 2,
}

export enum TreeItemCheckboxState {
  Unchecked = 0,
  Checked = 1,
}

export enum ViewColumn {
  Active = -1,
  Beside = -2,
  One = 1,
  Two = 2,
  Three = 3,
  Four = 4,
  Five = 5,
  Six = 6,
  Seven = 7,
  Eight = 8,
  Nine = 9,
}

export enum ConfigurationTarget {
  Global = 1,
  Workspace = 2,
  WorkspaceFolder = 3,
}

export enum QuickPickItemKind {
  Separator = -1,
  Default = 0,
}

export enum UIKind {
  Desktop = 1,
  Web = 2,
}

export enum LogLevel {
  Off = 0,
  Trace = 1,
  Debug = 2,
  Info = 3,
  Warning = 4,
  Error = 5,
}

export enum ExtensionMode {
  Production = 1,
  Development = 2,
  Test = 3,
}

export enum ExtensionKind {
  UI = 1,
  Workspace = 2,
}

export enum ColorThemeKind {
  Light = 1,
  Dark = 2,
  HighContrast = 3,
  HighContrastLight = 4,
}

export enum FileType {
  Unknown = 0,
  File = 1,
  Directory = 2,
  SymbolicLink = 64,
}

export enum ProgressLocation {
  SourceControl = 1,
  Window = 10,
  Notification = 15,
}

export enum TextEditorRevealType {
  Default = 0,
  InCenter = 1,
  InCenterIfOutsideViewport = 2,
  AtTop = 3,
}

export enum DecorationRangeBehavior {
  OpenOpen = 0,
  ClosedClosed = 1,
  OpenClosed = 2,
  ClosedOpen = 3,
}

export enum OverviewRulerLane {
  Left = 1,
  Center = 2,
  Right = 4,
  Full = 7,
}

export enum DiagnosticSeverity {
  Error = 0,
  Warning = 1,
  Information = 2,
  Hint = 3,
}

function safeDecode(value: string): string {
  try {
    return decodeURIComponent(value);
  } catch {
    return value;
  }
}

function encodePath(path: string): string {
  return path
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/");
}

export class Uri {
  constructor(
    readonly scheme: string,
    readonly authority: string,
    readonly path: string,
    readonly query = "",
    readonly fragment = "",
  ) {}

  static parse(value: string): Uri {
    const url = new URL(value);
    return new Uri(
      url.protocol.slice(0, -1),
      url.host,
      safeDecode(url.pathname),
      url.search.slice(1),
      url.hash.slice(1),
    );
  }

  static file(path: string): Uri {
    const normalized = path.startsWith("/") ? path : `/${path}`;
    return new Uri("file", "", normalized);
  }

  static joinPath(base: Uri, ...segments: string[]): Uri {
    return base.with({ path: posix.join(base.path, ...segments) });
  }

  get fsPath(): string {
    if (this.scheme !== "file") return this.path;
    if (process.platform === "win32" && /^\/[A-Za-z]:/.test(this.path)) {
      return this.path.slice(1).replace(/\//g, "\\");
    }
    return this.path;
  }

  with(change: {
    scheme?: string;
    authority?: string;
    path?: string;
    query?: string;
    fragment?: string;
  }): Uri {
    return new Uri(
      change.scheme ?? this.scheme,
      change.authority ?? this.authority,
      change.path ?? this.path,
      change.query ?? this.query,
      change.fragment ?? this.fragment,
    );
  }

  toString(skipEncoding = false): string {
    const path = skipEncoding ? this.path : encodePath(this.path);
    const query = this.query
      ? `?${skipEncoding ? this.query : encodeURIComponent(this.query)}`
      : "";
    const fragment = this.fragment
      ? `#${skipEncoding ? this.fragment : encodeURIComponent(this.fragment)}`
      : "";
    if (this.scheme === "file") return `file://${path}${query}${fragment}`;
    return `${this.scheme}://${this.authority}${path}${query}${fragment}`;
  }

  toJSON(): string {
    return this.toString();
  }
}

export class Position {
  constructor(
    readonly line: number,
    readonly character: number,
  ) {}

  isBefore(other: Position): boolean {
    return this.line < other.line || (this.line === other.line && this.character < other.character);
  }

  isAfter(other: Position): boolean {
    return other.isBefore(this);
  }

  isEqual(other: Position): boolean {
    return this.line === other.line && this.character === other.character;
  }

  translate(lineDelta = 0, characterDelta = 0): Position {
    return new Position(this.line + lineDelta, this.character + characterDelta);
  }

  with(line = this.line, character = this.character): Position {
    return new Position(line, character);
  }

  compareTo(other: Position): number {
    if (this.isBefore(other)) return -1;
    if (this.isAfter(other)) return 1;
    return 0;
  }
}

export class Range {
  constructor(
    readonly start: Position,
    readonly end: Position,
  ) {}

  get isEmpty(): boolean {
    return this.start.isEqual(this.end);
  }

  get isSingleLine(): boolean {
    return this.start.line === this.end.line;
  }

  contains(positionOrRange: Position | Range): boolean {
    if (positionOrRange instanceof Range) {
      return this.contains(positionOrRange.start) && this.contains(positionOrRange.end);
    }
    return !positionOrRange.isBefore(this.start) && !positionOrRange.isAfter(this.end);
  }

  with(start = this.start, end = this.end): Range {
    return new Range(start, end);
  }
}

export class Selection extends Range {
  constructor(
    readonly anchor: Position,
    readonly active: Position,
  ) {
    super(
      new Position(Math.min(anchor.line, active.line), Math.min(anchor.character, active.character)),
      new Position(Math.max(anchor.line, active.line), Math.max(anchor.character, active.character)),
    );
  }

  get isReversed(): boolean {
    return this.anchor.isAfter(this.active);
  }
}

export class Location {
  constructor(
    readonly uri: Uri,
    readonly range: Range,
  ) {}
}

export class Diagnostic {
  source?: string;
  code?: string | number;
  relatedInformation?: unknown[];

  constructor(
    readonly range: Range,
    readonly message: string,
    readonly severity: DiagnosticSeverity = DiagnosticSeverity.Error,
  ) {}
}

export class RelativePattern {
  readonly base: string;
  readonly baseUri: Uri;
  readonly pattern: string;

  constructor(base: string | Uri | { uri: Uri }, pattern: string) {
    this.baseUri =
      typeof base === "string" ? Uri.file(base) : base instanceof Uri ? base : base.uri;
    this.base = this.baseUri.fsPath;
    this.pattern = pattern;
  }
}

export class CancellationTokenSource {
  private emitter = new EventEmitter<void>();
  private cancelled = false;

  readonly token = {
    get isCancellationRequested(): boolean {
      return false;
    },
    onCancellationRequested: this.emitter.event,
  };

  cancel(): void {
    if (this.cancelled) return;
    this.cancelled = true;
    this.emitter.fire(undefined);
  }

  dispose(): void {
    this.emitter.dispose();
  }
}

export const CancellationToken = {
  None: {
    isCancellationRequested: false,
    onCancellationRequested: (): Disposable => new Disposable(),
  },
};

export class ThemeIcon {
  constructor(
    readonly id: string,
    readonly color?: unknown,
  ) {}
}

export class ThemeColor {
  constructor(readonly id: string) {}
}

export class McpHttpServerDefinition {
  headers: Record<string, string>;

  constructor(
    readonly label: string,
    readonly uri: Uri,
    headers?: Record<string, string>,
    readonly version?: string,
  ) {
    this.headers = headers ?? {};
  }
}

export class McpStdioServerDefinition {
  constructor(
    readonly label: string,
    readonly command: string,
    readonly args: string[] = [],
    readonly env?: Record<string, string | number | null>,
    readonly version?: string,
  ) {}
}

export class MarkdownString {
  value: string;
  isTrusted = false;
  supportThemeIcons = false;
  supportHtml = false;
  baseUri?: Uri;

  constructor(value = "", supportThemeIcons = false) {
    this.value = value;
    this.supportThemeIcons = supportThemeIcons;
  }

  appendText(value: string): MarkdownString {
    this.value += value;
    return this;
  }

  appendMarkdown(value: string): MarkdownString {
    this.value += value;
    return this;
  }

  appendCodeblock(value: string, language = ""): MarkdownString {
    this.value += `\n\`\`\`${language}\n${value}\n\`\`\`\n`;
    return this;
  }
}

export class TreeItem {
  label: string | { label: string };
  collapsibleState: TreeItemCollapsibleState;
  id?: string;
  iconPath?: unknown;
  description?: string | boolean;
  contextValue?: string;
  tooltip?: string | MarkdownString;
  command?: unknown;
  resourceUri?: Uri;
  checkboxState?: unknown;
  accessibilityInformation?: unknown;

  constructor(
    label: string | { label: string },
    collapsibleState: TreeItemCollapsibleState = TreeItemCollapsibleState.None,
  ) {
    this.label = label;
    this.collapsibleState = collapsibleState;
  }
}

export class StatusBarItem {
  readonly id: string;
  text = "";
  tooltip?: string | MarkdownString;
  color?: unknown;
  backgroundColor?: unknown;
  command?: string | { command: string; arguments?: unknown[] };
  name?: string;
  accessibilityInformation?: unknown;
  visible = false;

  show = vi.fn(() => {
    this.visible = true;
  });

  hide = vi.fn(() => {
    this.visible = false;
  });

  dispose = vi.fn(() => {
    this.visible = false;
  });

  constructor(
    readonly alignment: StatusBarAlignment = StatusBarAlignment.Left,
    readonly priority = 0,
  ) {
    this.id = `statusbar-${Date.now()}-${Math.random().toString(36).slice(2)}`;
  }
}

export const createdStatusBarItems: StatusBarItem[] = [];

export class MockOutputChannel {
  readonly lines: string[] = [];

  append = vi.fn((value: string) => {
    this.lines.push(value);
  });

  appendLine = vi.fn((value: string) => {
    this.lines.push(value);
  });

  replace = vi.fn((value: string) => {
    this.lines.splice(0, this.lines.length, value);
  });

  clear = vi.fn(() => {
    this.lines.length = 0;
  });

  show = vi.fn();
  hide = vi.fn();
  dispose = vi.fn();

  constructor(readonly name: string) {}
}

export const createdOutputChannels: MockOutputChannel[] = [];

class MockWebview {
  html = "";
  options: Record<string, unknown> = {};
  cspSource = "vscode-resource:";
  private receiveEmitter = new EventEmitter<unknown>();

  readonly onDidReceiveMessage = this.receiveEmitter.event;

  postMessage = vi.fn(async () => true);

  asWebviewUri = vi.fn((uri: Uri) => uri);

  fireMessage(message: unknown): void {
    this.receiveEmitter.fire(message);
  }
}

class MockWebviewPanel {
  title: string;
  webview = new MockWebview();
  viewColumn: ViewColumn;
  visible = true;
  active = true;
  private disposed = false;
  private disposeEmitter = new EventEmitter<void>();
  private viewStateEmitter = new EventEmitter<MockWebviewPanel>();

  readonly onDidDispose = this.disposeEmitter.event;
  readonly onDidChangeViewState = this.viewStateEmitter.event;

  reveal = vi.fn();

  dispose = vi.fn(() => {
    if (this.disposed) return;
    this.disposed = true;
    this.visible = false;
    this.active = false;
    this.disposeEmitter.fire(undefined);
  });

  constructor(
    readonly viewType: string,
    title: string,
    showOptions: ViewColumn | { viewColumn: ViewColumn; preserveFocus?: boolean },
    options?: Record<string, unknown>,
  ) {
    this.title = title;
    this.viewColumn =
      typeof showOptions === "number" ? showOptions : showOptions.viewColumn;
    this.webview.options = options ?? {};
  }
}

export const createdWebviewPanels: MockWebviewPanel[] = [];

class MockTerminal {
  readonly name: string;
  readonly processId = Promise.resolve(12345);
  readonly creationOptions: unknown;
  readonly state = { isInteractedWith: false };
  readonly shellIntegration = undefined;

  sendText = vi.fn();
  show = vi.fn();
  hide = vi.fn();
  dispose = vi.fn();

  constructor(name: string, options: unknown) {
    this.name = name;
    this.creationOptions = options;
  }
}

export const createdTerminals: MockTerminal[] = [];

class MockTreeView {
  readonly onDidChangeSelection = new EventEmitter<unknown>().event;
  readonly onDidChangeVisibility = new EventEmitter<unknown>().event;
  readonly onDidChangeCheckboxState = new EventEmitter<unknown>().event;
  readonly message?: string;
  readonly title?: string;
  readonly description?: string;
  selection: readonly unknown[] = [];
  visible = true;

  reveal = vi.fn(async () => undefined);
  dispose = vi.fn();

  constructor(readonly viewId: string) {}
}

const treeDataProviders = new Map<string, unknown>();
const treeViews = new Map<string, MockTreeView>();
const webviewViewProviders = new Map<string, unknown>();

export function getTreeDataProvider(viewId: string): unknown {
  return treeDataProviders.get(viewId);
}

export function getTreeView(viewId: string): MockTreeView | undefined {
  return treeViews.get(viewId);
}

export function getWebviewViewProvider(viewId: string): unknown {
  return webviewViewProviders.get(viewId);
}

export interface ConfigurationChangeEvent {
  affectsConfiguration(section: string, scope?: unknown): boolean;
}

const configurationValues = new Map<string, unknown>();
const configurationChanged = new EventEmitter<ConfigurationChangeEvent>();

export function setMockConfiguration(values: Record<string, unknown>): void {
  for (const [key, value] of Object.entries(values)) configurationValues.set(key, value);
}

class MockWorkspaceConfiguration {
  constructor(private readonly section: string) {}

  private fullKey(key: string): string {
    return this.section ? `${this.section}.${key}` : key;
  }

  get<T>(key: string, defaultValue?: T): T | undefined {
    const full = this.fullKey(key);
    return (configurationValues.has(full) ? configurationValues.get(full) : defaultValue) as
      | T
      | undefined;
  }

  has(key: string): boolean {
    return configurationValues.has(this.fullKey(key));
  }

  inspect<T>(key: string): {
    key: string;
    defaultValue?: T;
    globalValue?: T;
    workspaceValue?: T;
    workspaceFolderValue?: T;
  } {
    return { key: this.fullKey(key) };
  }

  async update(key: string, value: unknown, _target?: ConfigurationTarget): Promise<void> {
    const full = this.fullKey(key);
    if (value === undefined) configurationValues.delete(full);
    else configurationValues.set(full, value);
    configurationChanged.fire({
      affectsConfiguration: (section) =>
        section === "" ||
        full === section ||
        full.startsWith(`${section}.`) ||
        section.startsWith(`${full}.`),
    });
  }
}

export class MockMemento {
  private store = new Map<string, unknown>();

  get<T>(key: string, defaultValue?: T): T | undefined {
    return (this.store.has(key) ? this.store.get(key) : defaultValue) as T | undefined;
  }

  update(key: string, value: unknown): Promise<void> {
    this.store.set(key, value);
    return Promise.resolve();
  }

  keys = vi.fn((): readonly string[] => [...this.store.keys()]);

  setKeysForSync = vi.fn();
}

export interface WorkspaceFolder {
  readonly uri: Uri;
  readonly name: string;
  readonly index: number;
}

let workspaceFoldersValue: WorkspaceFolder[] | undefined;
let trustedValue = true;
let activeTextEditorValue: unknown;
const workspaceFoldersChanged = new EventEmitter<{
  added: WorkspaceFolder[];
  removed: WorkspaceFolder[];
}>();

const fileData = new Map<string, Uint8Array>();
const directories = new Set<string>();

function uriKey(uri: Uri): string {
  return uri.toString();
}

function fileNotFound(uri: Uri): Error & { code: string } {
  const error = new Error(`FileNotFound: ${uri.fsPath}`) as Error & { code: string };
  error.code = "FileNotFound";
  return error;
}

function deletePath(key: string): void {
  fileData.delete(key);
  directories.delete(key);
  const prefix = `${key}/`;
  for (const candidate of [...fileData.keys()]) {
    if (candidate.startsWith(prefix)) fileData.delete(candidate);
  }
  for (const candidate of [...directories]) {
    if (candidate.startsWith(prefix)) directories.delete(candidate);
  }
}

function createMockTextDocument(uri: Uri) {
  return {
    uri,
    fileName: uri.fsPath,
    isUntitled: false,
    languageId: "plaintext",
    version: 1,
    isDirty: false,
    isClosed: false,
    eol: 1,
    lineCount: 0,
    save: vi.fn(async () => true),
    getText: vi.fn(() => ""),
    lineAt: vi.fn(() => ({ text: "", range: new Range(new Position(0, 0), new Position(0, 0)) })),
    offsetAt: vi.fn(() => 0),
    positionAt: vi.fn(() => new Position(0, 0)),
    validateRange: vi.fn((range: Range) => range),
    validatePosition: vi.fn((position: Position) => position),
  };
}

function createMockTextEditor(document = createMockTextDocument(Uri.file("/mock.txt"))) {
  return {
    document,
    selection: new Selection(new Position(0, 0), new Position(0, 0)),
    selections: [],
    visibleRanges: [],
    options: {},
    viewColumn: ViewColumn.One,
    edit: vi.fn(async () => true),
    insertSnippet: vi.fn(async () => true),
    setDecorations: vi.fn(),
    revealRange: vi.fn(),
    show: vi.fn(),
    hide: vi.fn(),
  };
}

function createFileSystemWatcher() {
  const createEmitter = new EventEmitter<Uri>();
  const changeEmitter = new EventEmitter<Uri>();
  const deleteEmitter = new EventEmitter<Uri>();
  return {
    ignoreCreateEvents: false,
    ignoreChangeEvents: false,
    ignoreDeleteEvents: false,
    onDidCreate: createEmitter.event,
    onDidChange: changeEmitter.event,
    onDidDelete: deleteEmitter.event,
    dispose: vi.fn(),
  };
}

export const workspace = {
  get workspaceFolders(): WorkspaceFolder[] | undefined {
    return workspaceFoldersValue;
  },
  set workspaceFolders(value: WorkspaceFolder[] | undefined) {
    workspaceFoldersValue = value;
  },
  get name(): string {
    return workspaceFoldersValue?.[0]?.name ?? "";
  },
  get rootPath(): string | undefined {
    return workspaceFoldersValue?.[0]?.uri.fsPath;
  },
  workspaceFile: undefined as Uri | undefined,
  get isTrusted(): boolean {
    return trustedValue;
  },
  set isTrusted(value: boolean) {
    trustedValue = value;
  },
  onDidChangeConfiguration: configurationChanged.event,
  onDidChangeWorkspaceFolders: workspaceFoldersChanged.event,
  onDidGrantWorkspaceTrust: new EventEmitter<void>().event,
  onDidChangeTextDocument: new EventEmitter<unknown>().event,
  onDidOpenTextDocument: new EventEmitter<unknown>().event,
  onDidCloseTextDocument: new EventEmitter<unknown>().event,
  onDidSaveTextDocument: new EventEmitter<unknown>().event,
  onWillSaveTextDocument: new EventEmitter<unknown>().event,
  textDocuments: [] as unknown[],
  workspaceState: new MockMemento(),
  globalState: new MockMemento(),

  getConfiguration: vi.fn(
    (section?: string, _scope?: unknown) => new MockWorkspaceConfiguration(section ?? ""),
  ),

  getWorkspaceFolder: vi.fn((uri: Uri): WorkspaceFolder | undefined =>
    workspaceFoldersValue?.find(
      (folder) =>
        uri.fsPath === folder.uri.fsPath || uri.fsPath.startsWith(`${folder.uri.fsPath}/`),
    ),
  ),

  asRelativePath: vi.fn((pathOrUri: string | Uri, includeWorkspaceFolder?: boolean): string => {
    const fsPath = typeof pathOrUri === "string" ? pathOrUri : pathOrUri.fsPath;
    const folder = workspaceFoldersValue?.[0];
    if (!folder) return fsPath;
    const root = folder.uri.fsPath;
    if (fsPath === root) return includeWorkspaceFolder ? folder.name : "";
    if (!fsPath.startsWith(`${root}/`)) return fsPath;
    const relative = fsPath.slice(root.length + 1);
    return includeWorkspaceFolder ? `${folder.name}/${relative}` : relative;
  }),

  openTextDocument: vi.fn(async (pathOrUri: string | Uri) =>
    createMockTextDocument(typeof pathOrUri === "string" ? Uri.file(pathOrUri) : pathOrUri),
  ),

  findFiles: vi.fn(async () => [] as Uri[]),
  createFileSystemWatcher: vi.fn(() => createFileSystemWatcher()),
  applyEdit: vi.fn(async () => true),
  saveAll: vi.fn(async () => true),
  updateWorkspaceFolders: vi.fn(() => true),
  registerTextDocumentContentProvider: vi.fn(() => new Disposable()),
  registerFileSystemProvider: vi.fn(() => new Disposable()),
  registerTaskProvider: vi.fn(() => new Disposable()),

  fs: {
    readFile: vi.fn(async (uri: Uri): Promise<Uint8Array> => {
      const data = fileData.get(uriKey(uri));
      if (!data) throw fileNotFound(uri);
      return new Uint8Array(data);
    }),

    writeFile: vi.fn(async (uri: Uri, content: Uint8Array): Promise<void> => {
      fileData.set(uriKey(uri), new Uint8Array(content));
    }),

    stat: vi.fn(async (uri: Uri) => {
      const data = fileData.get(uriKey(uri));
      if (data) {
        return {
          type: FileType.File,
          ctime: 0,
          mtime: 0,
          size: data.byteLength,
          permissions: undefined,
        };
      }
      if (directories.has(uriKey(uri))) {
        return { type: FileType.Directory, ctime: 0, mtime: 0, size: 0, permissions: undefined };
      }
      throw fileNotFound(uri);
    }),

    readDirectory: vi.fn(async (uri: Uri): Promise<[string, FileType][]> => {
      const key = uriKey(uri);
      const prefix = `${key}/`;
      const entries = new Map<string, FileType>();
      for (const candidate of fileData.keys()) {
        if (!candidate.startsWith(prefix)) continue;
        const rest = candidate.slice(prefix.length);
        if (rest && !rest.includes("/")) entries.set(rest, FileType.File);
      }
      for (const candidate of directories) {
        if (!candidate.startsWith(prefix)) continue;
        const rest = candidate.slice(prefix.length);
        if (rest && !rest.includes("/")) entries.set(rest, FileType.Directory);
      }
      return [...entries.entries()].sort(([left], [right]) => left.localeCompare(right));
    }),

    createDirectory: vi.fn(async (uri: Uri): Promise<void> => {
      directories.add(uriKey(uri));
    }),

    delete: vi.fn(async (uri: Uri): Promise<void> => {
      deletePath(uriKey(uri));
    }),

    rename: vi.fn(async (source: Uri, target: Uri, _options?: unknown): Promise<void> => {
      const sourceKey = uriKey(source);
      const targetKey = uriKey(target);
      const data = fileData.get(sourceKey);
      if (data) {
        fileData.set(targetKey, data);
        fileData.delete(sourceKey);
      }
      if (directories.has(sourceKey)) {
        directories.add(targetKey);
        directories.delete(sourceKey);
      }
    }),

    copy: vi.fn(async (source: Uri, target: Uri, _options?: unknown): Promise<void> => {
      const sourceKey = uriKey(source);
      const targetKey = uriKey(target);
      const data = fileData.get(sourceKey);
      if (data) fileData.set(targetKey, new Uint8Array(data));
      if (directories.has(sourceKey)) directories.add(targetKey);
    }),
  },
};

export function setWorkspaceFolders(paths: string[] | undefined): void {
  workspaceFoldersValue = paths?.map((path, index) => ({
    uri: Uri.file(path),
    name: posix.basename(path),
    index,
  }));
  workspaceFoldersChanged.fire({
    added: workspaceFoldersValue ?? [],
    removed: [],
  });
}

export function setWorkspaceTrusted(value: boolean): void {
  trustedValue = value;
}

export const window = {
  get activeTextEditor(): unknown {
    return activeTextEditorValue;
  },
  set activeTextEditor(value: unknown) {
    activeTextEditorValue = value;
  },
  visibleTextEditors: [] as unknown[],
  get terminals(): MockTerminal[] {
    return createdTerminals;
  },
  state: { focused: true, active: true },
  onDidChangeActiveTextEditor: new EventEmitter<unknown>().event,
  onDidChangeTextEditorSelection: new EventEmitter<unknown>().event,
  onDidChangeVisibleTextEditors: new EventEmitter<unknown[]>().event,
  onDidChangeWindowState: new EventEmitter<{ focused: boolean }>().event,

  createStatusBarItem: vi.fn(
    (alignment: StatusBarAlignment = StatusBarAlignment.Left, priority = 0) => {
      const item = new StatusBarItem(alignment, priority);
      createdStatusBarItems.push(item);
      return item;
    },
  ),

  registerTreeDataProvider: vi.fn((viewId: string, provider: unknown) => {
    treeDataProviders.set(viewId, provider);
    return new Disposable(() => treeDataProviders.delete(viewId));
  }),

  createTreeView: vi.fn((viewId: string, options?: { treeDataProvider?: unknown }) => {
    const tree = new MockTreeView(viewId);
    treeDataProviders.set(viewId, options?.treeDataProvider);
    treeViews.set(viewId, tree);
    return tree;
  }),

  registerWebviewViewProvider: vi.fn(
    (viewId: string, provider: unknown, _options?: unknown) => {
      webviewViewProviders.set(viewId, provider);
      return new Disposable(() => webviewViewProviders.delete(viewId));
    },
  ),

  registerWebviewPanelSerializer: vi.fn(() => new Disposable()),
  registerUriHandler: vi.fn(() => new Disposable()),

  showInformationMessage: vi.fn(async (..._args: unknown[]) => undefined as string | undefined),
  showWarningMessage: vi.fn(async (..._args: unknown[]) => undefined as string | undefined),
  showErrorMessage: vi.fn(async (..._args: unknown[]) => undefined as string | undefined),
  showQuickPick: vi.fn(async (..._args: unknown[]) => undefined as unknown),
  showInputBox: vi.fn(async (..._args: unknown[]) => undefined as string | undefined),
  showOpenDialog: vi.fn(async (..._args: unknown[]) => undefined),
  showSaveDialog: vi.fn(async (..._args: unknown[]) => undefined),
  showWorkspaceFolderPick: vi.fn(async (..._args: unknown[]) => undefined),
  showTextDocument: vi.fn(async (..._args: unknown[]) => createMockTextEditor()),

  createOutputChannel: vi.fn((name: string, _options?: unknown) => {
    const channel = new MockOutputChannel(name);
    createdOutputChannels.push(channel);
    return channel;
  }),

  createWebviewPanel: vi.fn(
    (
      viewType: string,
      title: string,
      showOptions: ViewColumn | { viewColumn: ViewColumn; preserveFocus?: boolean },
      options?: Record<string, unknown>,
    ) => {
      const panel = new MockWebviewPanel(viewType, title, showOptions, options);
      createdWebviewPanels.push(panel);
      return panel;
    },
  ),

  createTerminal: vi.fn((options: string | { name?: string }) => {
    const name = typeof options === "string" ? options : (options.name ?? "terminal");
    const terminal = new MockTerminal(name, options);
    createdTerminals.push(terminal);
    return terminal;
  }),

  createTextEditorDecorationType: vi.fn((_options: unknown) => ({
    key: "mock-decoration",
    dispose: vi.fn(),
  })),

  setStatusBarMessage: vi.fn((_text: string, _timeout?: number) => new Disposable()),

  withProgress: vi.fn(
    async (
      _options: unknown,
      task: (progress: { report: (value: unknown) => void }, token: unknown) => unknown,
    ) => {
      const source = new CancellationTokenSource();
      try {
        return await task({ report: vi.fn() }, source.token);
      } finally {
        source.dispose();
      }
    },
  ),
};

const commandRegistry = new Map<string, (...args: unknown[]) => unknown>();

export const commands = {
  registerCommand: vi.fn(
    (id: string, callback: (...args: unknown[]) => unknown) => {
      commandRegistry.set(id, callback);
      return new Disposable(() => {
        if (commandRegistry.get(id) === callback) commandRegistry.delete(id);
      });
    },
  ),

  registerTextEditorCommand: vi.fn(
    (id: string, callback: (...args: unknown[]) => unknown) => {
      commandRegistry.set(id, (...args: unknown[]) => {
        const editor = activeTextEditorValue ?? createMockTextEditor();
        return callback(editor, { replace: vi.fn(), insert: vi.fn() }, ...args);
      });
      return new Disposable(() => commandRegistry.delete(id));
    },
  ),

  executeCommand: vi.fn(async (id: string, ...args: unknown[]) => {
    const callback = commandRegistry.get(id);
    if (!callback) throw new Error(`command '${id}' not found`);
    return callback(...args);
  }),

  getCommands: vi.fn(async (_filterInternal?: boolean) => [...commandRegistry.keys()]),
};

const mcpServerDefinitionProviders = new Map<string, unknown>();

export function getMcpServerDefinitionProvider(id: string): unknown {
  return mcpServerDefinitionProviders.get(id);
}

export const lm = {
  registerMcpServerDefinitionProvider: vi.fn((id: string, provider: unknown) => {
    mcpServerDefinitionProviders.set(id, provider);
    return new Disposable(() => mcpServerDefinitionProviders.delete(id));
  }),
  registerTool: vi.fn(() => new Disposable()),
  selectChatModels: vi.fn(async () => [] as unknown[]),
  onDidChangeChatModels: new EventEmitter<void>().event,
};

export const l10n = {
  t: vi.fn((message: string, ...args: Array<string | number | boolean>) =>
    message.replace(/\{(\d+)\}/g, (match, index: string) => {
      const value = args[Number(index)];
      return value === undefined ? match : String(value);
    }),
  ),
  bundle: undefined as unknown,
  uri: undefined as Uri | undefined,
};

let clipboardText = "";

export const env = {
  language: "en",
  appName: "Visual Studio Code",
  appRoot: process.cwd(),
  appHost: "desktop",
  machineId: "mock-machine-id",
  sessionId: "mock-session-id",
  uriScheme: "vscode",
  shell: "/bin/zsh",
  remoteName: undefined as string | undefined,
  uiKind: UIKind.Desktop,
  logLevel: LogLevel.Info,
  isTelemetryEnabled: false,
  isNewAppInstall: false,
  onDidChangeTelemetryEnabled: new EventEmitter<boolean>().event,
  onDidChangeShell: new EventEmitter<string>().event,
  clipboard: {
    readText: vi.fn(async () => clipboardText),
    writeText: vi.fn(async (value: string) => {
      clipboardText = value;
    }),
  },
  openExternal: vi.fn(async (_uri: Uri) => true),
  asExternalUri: vi.fn(async (uri: Uri) => uri),
};

export class MockExtension {
  isActive: boolean;
  readonly exports: unknown;
  readonly packageJSON: Record<string, unknown>;
  readonly extensionPath: string;
  readonly extensionKind = ExtensionKind.Workspace;

  constructor(
    readonly id: string,
    extensionPath: string,
    options: { isActive?: boolean; exports?: unknown; packageJSON?: Record<string, unknown> } = {},
  ) {
    this.extensionPath = extensionPath;
    this.isActive = options.isActive ?? false;
    this.exports = options.exports ?? {};
    this.packageJSON = options.packageJSON ?? {};
  }

  get extensionUri(): Uri {
    return Uri.file(this.extensionPath);
  }

  async activate(): Promise<unknown> {
    this.isActive = true;
    return this.exports;
  }
}

const extensionMap = new Map<string, MockExtension>();

export function registerMockExtension(
  id: string,
  options: { isActive?: boolean; exports?: unknown; packageJSON?: Record<string, unknown> } = {},
): MockExtension {
  const extension = new MockExtension(id, process.cwd(), options);
  extensionMap.set(id, extension);
  return extension;
}

registerMockExtension("hpa.workbench", {
  packageJSON: { name: "workbench", publisher: "hpa", version: "0.1.0" },
});

export const extensions = {
  getExtension: vi.fn((id: string) => extensionMap.get(id)),
  get all(): readonly MockExtension[] {
    return [...extensionMap.values()];
  },
  onDidChange: new EventEmitter<void>().event,
};

const secretChangeEmitter = new EventEmitter<{ key: string }>();

export class MockSecretStorage {
  private values = new Map<string, string>();

  readonly onDidChange = secretChangeEmitter.event;

  get(key: string): Promise<string | undefined> {
    return Promise.resolve(this.values.get(key));
  }

  store(key: string, value: string): Promise<void> {
    this.values.set(key, value);
    secretChangeEmitter.fire({ key });
    return Promise.resolve();
  }

  delete(key: string): Promise<void> {
    this.values.delete(key);
    secretChangeEmitter.fire({ key });
    return Promise.resolve();
  }

  clear(): void {
    this.values.clear();
  }
}

export const secrets = new MockSecretStorage();

export function createSecretStorage(): MockSecretStorage {
  return new MockSecretStorage();
}

export const languages = {
  createDiagnosticCollection: vi.fn((name?: string) => {
    const entries = new Map<string, unknown>();
    return {
      name: name ?? "",
      set: vi.fn((uri: Uri, diagnostics: unknown) => entries.set(uri.toString(), diagnostics)),
      get: vi.fn((uri: Uri) => entries.get(uri.toString())),
      has: vi.fn((uri: Uri) => entries.has(uri.toString())),
      delete: vi.fn((uri: Uri) => entries.delete(uri.toString())),
      clear: vi.fn(() => entries.clear()),
      forEach: vi.fn(),
      dispose: vi.fn(),
    };
  }),
  registerCodeActionsProvider: vi.fn(() => new Disposable()),
  registerHoverProvider: vi.fn(() => new Disposable()),
  registerCompletionItemProvider: vi.fn(() => new Disposable()),
  registerDocumentSymbolProvider: vi.fn(() => new Disposable()),
  registerDefinitionProvider: vi.fn(() => new Disposable()),
  setLanguageConfiguration: vi.fn(() => new Disposable()),
  match: vi.fn(() => null),
};

export function resetMockVscode(): void {
  commandRegistry.clear();
  configurationValues.clear();
  fileData.clear();
  directories.clear();
  treeDataProviders.clear();
  treeViews.clear();
  webviewViewProviders.clear();
  mcpServerDefinitionProviders.clear();
  createdStatusBarItems.length = 0;
  createdOutputChannels.length = 0;
  createdWebviewPanels.length = 0;
  createdTerminals.length = 0;
  secretChangeEmitter.dispose();
  secrets.clear();
  workspace.workspaceFolders = undefined;
  workspace.isTrusted = true;
  window.activeTextEditor = undefined;
  env.language = "en";
  clipboardText = "";
  extensionMap.clear();
  registerMockExtension("hpa.workbench", {
    packageJSON: { name: "workbench", publisher: "hpa", version: "0.1.0" },
  });
}
