import path from 'node:path';
import * as vscode from 'vscode';
import { HighlightEngine, type HighlightDocument, tokenModifiers, tokenTypes } from './highlighting';
import { ServerManager } from './servers';

class HighlightProvider implements vscode.DocumentSemanticTokensProvider, vscode.Disposable {
  private documents = new Map<string, { version: number; parsed: HighlightDocument }>();
  private changed = new vscode.EventEmitter<void>();
  readonly onDidChangeSemanticTokens = this.changed.event;
  private subscriptions: vscode.Disposable[];

  constructor(private engine: HighlightEngine, private output: vscode.OutputChannel) {
    this.subscriptions = [
      vscode.workspace.onDidChangeTextDocument(event => {
        const key = event.document.uri.toString();
        const state = this.documents.get(key);
        if (!state || !event.contentChanges.length) return;
        try {
          state.parsed.update(event.contentChanges);
          state.version = event.document.version;
        } catch (error) {
          this.close(key);
          this.output.appendLine(`Highlight update failed: ${error}`);
        }
        this.changed.fire();
      }),
      vscode.workspace.onDidCloseTextDocument(document => this.close(document.uri.toString())),
    ];
  }

  provideDocumentSemanticTokens(document: vscode.TextDocument, cancellation: vscode.CancellationToken): vscode.SemanticTokens | undefined {
    if (cancellation.isCancellationRequested) return;
    const key = document.uri.toString();
    let state = this.documents.get(key);
    if (!state || state.version !== document.version) {
      this.close(key);
      state = { version: document.version, parsed: this.engine.open(document.getText()) };
      this.documents.set(key, state);
    }
    const builder = new vscode.SemanticTokensBuilder();
    for (const token of state.parsed.tokens()) {
      builder.push(token.line, token.start, token.length, tokenTypes.indexOf(token.type), token.modifiers);
    }
    return builder.build();
  }

  private close(key: string): void {
    this.documents.get(key)?.parsed.dispose();
    this.documents.delete(key);
  }

  dispose(): void {
    this.subscriptions.forEach(subscription => subscription.dispose());
    for (const key of this.documents.keys()) this.close(key);
    this.changed.dispose();
    this.engine.dispose();
  }
}

let servers: ServerManager | undefined;

export async function activate(context: vscode.ExtensionContext): Promise<void> {
  const output = vscode.window.createOutputChannel('Ruddy');
  context.subscriptions.push(output);
  try {
    const engine = await HighlightEngine.load(path.join(context.extensionPath, 'dist'));
    const provider = new HighlightProvider(engine, output);
    context.subscriptions.push(provider, vscode.languages.registerDocumentSemanticTokensProvider(
      { language: 'ruddy' }, provider, new vscode.SemanticTokensLegend(tokenTypes, tokenModifiers),
    ));
  } catch (error) {
    output.appendLine(`Could not load bundled Ruddy highlighter: ${error}`);
    void vscode.window.showErrorMessage('Ruddy highlighting could not load. Reinstall the extension; see the Ruddy output log for details.');
  }
  servers = new ServerManager(output);
  context.subscriptions.push(
    vscode.commands.registerCommand('ruddy.restartServer', () => servers?.refresh(true)),
    vscode.workspace.onDidChangeConfiguration(event => {
      if (event.affectsConfiguration('ruddy.server.path')) void servers?.refresh();
    }),
    vscode.workspace.onDidChangeWorkspaceFolders(() => { void servers?.refresh(true); }),
    vscode.workspace.onDidGrantWorkspaceTrust(() => { void servers?.refresh(); }),
  );
  const watcher = vscode.workspace.createFileSystemWatcher('**/Ruddy.toml');
  context.subscriptions.push(watcher,
    watcher.onDidCreate(() => { void servers?.refresh(); }),
    watcher.onDidDelete(() => { void servers?.refresh(); }),
  );
  await servers.refresh();
}

export async function deactivate(): Promise<void> {
  await servers?.dispose();
  servers = undefined;
}
