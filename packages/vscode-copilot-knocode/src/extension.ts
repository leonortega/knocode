import * as vscode from "vscode";
import { registerKnocodeParticipant } from "./participant";

export function activate(context: vscode.ExtensionContext): void {
  registerKnocodeParticipant(context);
}

export function deactivate(): void {
  // no-op
}