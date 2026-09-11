import assert from 'node:assert/strict';
import path from 'node:path';
import * as vscode from 'vscode';

export async function run(): Promise<void> {
  const root = process.env.RUDDY_TEST_ROOT!;
  const standalone = await vscode.workspace.openTextDocument(vscode.Uri.file(path.join(root, 'standalone/file.rud')));
  await vscode.window.showTextDocument(standalone);
  assert.equal(standalone.languageId, 'ruddy');
  await vscode.extensions.getExtension('lgatlin.ruddy')!.activate();
  const tokens = await vscode.commands.executeCommand<vscode.SemanticTokens>('vscode.provideDocumentSemanticTokens', standalone.uri);
  assert.ok(tokens && tokens.data.length > 0, 'standalone files have bundled Tree-sitter highlighting');
  console.log('PASS standalone highlighting in the VS Code extension host');

  const change = new vscode.WorkspaceEdit();
  change.insert(standalone.uri, new vscode.Position(0, 0), '(* 😀\ncomment *)\n');
  await vscode.workspace.applyEdit(change);
  const updated = await vscode.commands.executeCommand<vscode.SemanticTokens>('vscode.provideDocumentSemanticTokens', standalone.uri);
  assert.ok(updated && updated.data.length > tokens.data.length, 'edits refresh multiline highlighting');
  const standaloneHover = await hover(standalone.uri);
  assert.equal(standaloneHover.length, 0, 'a folder without Ruddy.toml has no server');
  console.log('PASS standalone edit highlighting without LSP');

  const names = ['first [project]', 'second', 'first [project]/nested'];
  const documents = await Promise.all(names.map(name => vscode.workspace.openTextDocument(vscode.Uri.file(path.join(root, name, 'lib.rud')))));
  if (process.env.RUDDY_TEST_UNTRUSTED === '1') {
    assert.equal(vscode.workspace.isTrusted, false);
    await vscode.commands.executeCommand('ruddy.restartServer');
    for (const document of documents) assert.equal((await hover(document.uri)).length, 0);
    console.log('PASS Restricted Mode highlighting with no LSP providers, including after restart');
    return;
  }
  assert.equal(vscode.workspace.isTrusted, true);
  for (const [index, document] of documents.entries()) {
    const hovers = await eventually(() => hover(document.uri), value => value.length > 0);
    assert.match(hovers.flatMap(h => h.contents.map(content => typeof content === 'string' ? content : content.value)).join('\n'), new RegExp(['Nat', 'Bool', 'String'][index]!));
    assert.equal(hovers.length, 1, 'nested project requests are not duplicated to the parent');
    const definitions = await vscode.commands.executeCommand<Array<vscode.Location | vscode.LocationLink>>('vscode.executeDefinitionProvider', document.uri, new vscode.Position(1, 15));
    assert.ok(definitions?.length);
    const definition = definitions[0]!;
    assert.equal(('uri' in definition ? definition.uri : definition.targetUri).toString(), document.uri.toString());
    const completions = await vscode.commands.executeCommand<vscode.CompletionList>('vscode.executeCompletionItemProvider', document.uri, new vscode.Position(1, 15));
    assert.ok(completions?.items.some(item => (typeof item.label === 'string' ? item.label : item.label.label) === 'value'));
  }
  console.log('PASS real LSP hover, definition and completion in sibling and nested project folders');

  for (const document of documents) {
    const edit = new vscode.WorkspaceEdit();
    edit.insert(document.uri, new vscode.Position(0, 3), '   ');
    await vscode.workspace.applyEdit(edit);
    const formatting = await eventually(
      () => vscode.commands.executeCommand<vscode.TextEdit[]>('vscode.executeFormatDocumentProvider', document.uri, { tabSize: 2, insertSpaces: true }),
      value => !!value?.length,
    );
    const formatted = new vscode.WorkspaceEdit();
    formatted.set(document.uri, formatting!);
    await vscode.workspace.applyEdit(formatted);
    assert.ok(document.getText().startsWith('let value = '));
  }
  console.log('PASS real LSP document formatting in every project folder');

  const first = documents[0]!;
  const bad = new vscode.WorkspaceEdit();
  bad.insert(first.uri, first.positionAt(first.getText().length), 'let broken = missing_name\n');
  await vscode.workspace.applyEdit(bad);
  await eventually(async () => vscode.languages.getDiagnostics(first.uri), value => value.some(d => d.severity === vscode.DiagnosticSeverity.Error));
  const repair = new vscode.WorkspaceEdit();
  repair.delete(first.uri, new vscode.Range(new vscode.Position(2, 0), first.positionAt(first.getText().length)));
  await vscode.workspace.applyEdit(repair);
  await eventually(async () => vscode.languages.getDiagnostics(first.uri), value => value.length === 0);
  console.log('PASS diagnostics appear and clear for unsaved edits');

  await vscode.commands.executeCommand('ruddy.restartServer');
  await eventually(() => hover(first.uri), value => value.length > 0);
  const settings = vscode.workspace.getConfiguration('ruddy', first.uri);
  const originalPath = settings.get<string>('server.path')!;
  await settings.update('server.path', path.join(root, 'missing-ruddy'), vscode.ConfigurationTarget.WorkspaceFolder);
  await eventually(() => hover(first.uri), value => value.length === 0);
  assert.ok((await hover(documents[1]!.uri)).length, 'changing one folder preserves the other server');
  assert.ok((await vscode.commands.executeCommand<vscode.SemanticTokens>('vscode.provideDocumentSemanticTokens', first.uri))?.data.length, 'highlighting survives a missing compiler');
  await settings.update('server.path', originalPath, vscode.ConfigurationTarget.WorkspaceFolder);
  await eventually(() => hover(first.uri), value => value.length > 0);
  console.log('PASS restart and recovery after changing the executable setting, with independent highlighting');
  const nested = documents[2]!;
  const nestedChange = new vscode.WorkspaceEdit();
  nestedChange.insert(nested.uri, new vscode.Position(0, 3), '   ');
  await vscode.workspace.applyEdit(nestedChange);
  const nestedFormatting = await vscode.commands.executeCommand<vscode.TextEdit[]>('vscode.executeFormatDocumentProvider', nested.uri, { tabSize: 2, insertSpaces: true });
  assert.ok(nestedFormatting?.length, 'nested formatting still works after the parent client registers last');
}

async function hover(uri: vscode.Uri): Promise<vscode.Hover[]> {
  return await vscode.commands.executeCommand<vscode.Hover[]>('vscode.executeHoverProvider', uri, new vscode.Position(1, 15)) ?? [];
}

async function eventually<T>(read: () => Thenable<T>, ready: (value: T) => boolean): Promise<T> {
  const deadline = Date.now() + 15_000;
  let value: T;
  do {
    value = await read();
    if (ready(value)) return value;
    await new Promise(resolve => setTimeout(resolve, 100));
  } while (Date.now() < deadline);
  assert.fail(`Timed out waiting for editor state; last value: ${JSON.stringify(value)}`);
}
