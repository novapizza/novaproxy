import { useState } from "react";
import { api } from "./api";

/** A copyable block of shell. Settings and the setup wizard both hand these out. */
export function CodeSnippet({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="code-block">
      <pre>{text}</pre>
      <button
        className="cb-copy"
        onClick={() => { void api.copyText(text); setCopied(true); setTimeout(() => setCopied(false), 1200); }}
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}
