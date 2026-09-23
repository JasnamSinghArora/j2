// minimal VS Code support, deliberately not LSP
const vscode = require("vscode");
const cp = require("child_process");
const path = require("path");

let diagnostics;

function jPath() {
  return vscode.workspace.getConfiguration("j2").get("path") || "j2";
}

// run active file in terminal; interpreter-first, instant
function runFile() {
  const ed = vscode.window.activeTextEditor;
  if (!ed || ed.document.languageId !== "j2") {
    vscode.window.showErrorMessage("J2: no .j2 file is active");
    return;
  }
  ed.document.save().then(() => {
    const file = ed.document.fileName;
    let term = vscode.window.terminals.find((t) => t.name === "J2");
    if (!term) term = vscode.window.createTerminal("J2");
    term.show(true);
    term.sendText(`${jPath()} ${shellQuote(file)}`);
  });
}

function shellQuote(s) {
  return "'" + s.replace(/'/g, "'\\''") + "'";
}

// check via `j2 emit-native`, parse error[Kind] lines
function check(doc) {
  if (!doc || doc.languageId !== "j2") return;
  if (!vscode.workspace.getConfiguration("j2").get("checkOnSave")) {
    diagnostics.delete(doc.uri);
    return;
  }
  const file = doc.fileName;
  cp.execFile(
    jPath(),
    ["emit-native", file],
    { cwd: path.dirname(file), timeout: 15000 },
    (err, _stdout, stderr) => {
      // ENOENT (j missing): clear, don't nag
      if (err && err.code === "ENOENT") {
        diagnostics.delete(doc.uri);
        return;
      }
      const out = [];
      const re = /error\[([^\]]+)\]:\s*([^\n]+)[\s\S]*?-->\s*[^\n]*?:(\d+):(\d+)/g;
      let m;
      while ((m = re.exec(stderr || "")) !== null) {
        const line = Math.max(0, parseInt(m[3], 10) - 1);
        const col = Math.max(0, parseInt(m[4], 10) - 1);
        const range = new vscode.Range(line, col, line, col + 1);
        out.push(
          new vscode.Diagnostic(range, `${m[1]}: ${m[2].trim()}`, vscode.DiagnosticSeverity.Error)
        );
      }
      diagnostics.set(doc.uri, out);
    }
  );
}

function activate(context) {
  diagnostics = vscode.languages.createDiagnosticCollection("j2");
  context.subscriptions.push(diagnostics);
  context.subscriptions.push(vscode.commands.registerCommand("j2.run", runFile));
  context.subscriptions.push(vscode.workspace.onDidSaveTextDocument(check));
  context.subscriptions.push(vscode.workspace.onDidOpenTextDocument(check));
  context.subscriptions.push(
    vscode.workspace.onDidCloseTextDocument((d) => diagnostics.delete(d.uri))
  );
  if (vscode.window.activeTextEditor) check(vscode.window.activeTextEditor.document);
}

function deactivate() {}

module.exports = { activate, deactivate };
