import * as vscode from 'vscode';
import { CloseAction, ErrorAction, LanguageClient, type Middleware, RevealOutputChannelOn } from 'vscode-languageclient/node';

interface Server {
  client: LanguageClient;
  command: string;
  stopping: boolean;
}

/** Serializes workspace/configuration changes with server startup and shutdown. */
export class ServerManager {
  private servers = new Map<string, Server>();
  private pending: Promise<void> = Promise.resolve();
  private disposed = false;

  constructor(private output: vscode.OutputChannel) {}

  refresh(restart = false): Promise<void> {
    this.pending = this.pending.then(() => this.reconcile(restart)).catch(error => {
      this.output.appendLine(`Could not update Ruddy servers: ${error}`);
    });
    return this.pending;
  }

  private async reconcile(restart: boolean): Promise<void> {
    if (this.disposed) return;
    const folders = vscode.workspace.isTrusted ? (vscode.workspace.workspaceFolders ?? []) : [];
    const desired = new Map<string, { folder: vscode.WorkspaceFolder; command: string }>();
    for (const folder of folders) {
      if (folder.uri.scheme !== 'file') continue;
      try {
        const stat = await vscode.workspace.fs.stat(vscode.Uri.joinPath(folder.uri, 'Ruddy.toml'));
        if (!(stat.type & vscode.FileType.File)) continue;
      } catch { continue; }
      const command = vscode.workspace.getConfiguration('ruddy', folder.uri).get<string>('server.path', 'ruddy');
      desired.set(folder.uri.toString(), { folder, command });
    }
    for (const [key, server] of this.servers) {
      if (restart || desired.get(key)?.command !== server.command) {
        await this.stop(server);
        this.servers.delete(key);
      }
    }
    for (const [key, { folder, command }] of desired) {
      if (this.disposed || !vscode.workspace.isTrusted) return;
      if (this.servers.has(key)) continue;
      const server = this.create(folder, command);
      this.servers.set(key, server);
      try {
        this.output.appendLine(`Starting ${folder.name}: ${command} lsp`);
        await server.client.start();
      } catch (error) {
        this.failure(folder, `Could not start ${JSON.stringify(command)} lsp: ${error}`);
      }
    }
  }

  private create(folder: vscode.WorkspaceFolder, command: string): Server {
    const server = { command, stopping: false } as Server;
    server.client = new LanguageClient('ruddy', `Ruddy (${folder.name})`,
      { command, args: ['lsp'], options: { cwd: folder.uri.fsPath, shell: false } },
      {
        // Escape glob syntax so names such as "project [draft]" match literally.
        documentSelector: [{ language: 'ruddy', scheme: 'file', pattern: `${folder.uri.fsPath.replace(/\\/g, '/').replace(/[?*\[\]{}]/g, '[$&]')}/**/*` }],
        workspaceFolder: folder,
        outputChannel: this.output,
        revealOutputChannelOn: RevealOutputChannelOn.Never,
        initializationFailedHandler: () => false,
        middleware: this.middleware(folder),
        errorHandler: {
          error: error => {
            this.output.appendLine(`${folder.name}: ${error}`);
            return { action: ErrorAction.Continue, handled: true };
          },
          closed: () => {
            if (!server.stopping && !this.disposed) this.failure(folder, 'The language server stopped unexpectedly.');
            return { action: CloseAction.DoNotRestart, handled: true };
          },
        },
      },
    );
    return server;
  }

  private middleware(folder: vscode.WorkspaceFolder): Middleware {
    // Explicit nested workspace folders belong to their own client, even
    // though their paths also match the parent client's document selector.
    const owns = (document: vscode.TextDocument) => vscode.workspace.getWorkspaceFolder(document.uri)?.uri.toString() === folder.uri.toString();
    return {
      didOpen: async (document, next) => { if (owns(document)) await next(document); },
      didChange: async (event, next) => { if (owns(event.document)) await next(event); },
      didSave: async (document, next) => { if (owns(document)) await next(document); },
      didClose: async (document, next) => { if (owns(document)) await next(document); },
      provideHover: (document, position, token, next) => owns(document) ? next(document, position, token) : null,
      provideCompletionItem: (document, position, context, token, next) => owns(document) ? next(document, position, context, token) : null,
      provideDefinition: (document, position, token, next) => owns(document) ? next(document, position, token) : null,
      provideDocumentFormattingEdits: (document, options, token, next) => owns(document) ? next(document, options, token) : [],
    };
  }

  private failure(folder: vscode.WorkspaceFolder, detail: string): void {
    this.output.appendLine(`${folder.name}: ${detail}`);
    void vscode.window.showErrorMessage(
      `Ruddy (${folder.name}): ${detail} Install Ruddy on this host (from the Ruddy repository: just install), or set ruddy.server.path.`,
      'Open Settings', 'Show Output', 'Restart',
    ).then(action => {
      if (action === 'Open Settings') void vscode.commands.executeCommand('workbench.action.openSettings', 'ruddy.server.path');
      if (action === 'Show Output') this.output.show();
      if (action === 'Restart') void this.refresh(true);
    });
  }

  async dispose(): Promise<void> {
    this.disposed = true;
    await this.pending;
    await Promise.all([...this.servers.values()].map(server => this.stop(server)));
    this.servers.clear();
  }

  private async stop(server: Server): Promise<void> {
    server.stopping = true;
    try { await server.client.dispose(); }
    catch (error) {
      // vscode-languageclient 9 rejects disposal in its startFailed state.
      // That state has no live process; it must not block a corrected path.
      this.output.appendLine(`Ruddy server cleanup: ${error}`);
    }
  }
}
