import { useState } from "react";
import PlanBadge from "../components/PlanBadge";
import { MANAGE_USAGE_URL, openExternal, startSignIn } from "../lib/tauri";

interface Props {
  onSignedIn: () => void;
  onExplore: () => void;
}

export default function Welcome({ onSignedIn, onExplore }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function handleContinue() {
    setBusy(true);
    setError(null);
    try {
      await startSignIn();
      onSignedIn();
    } catch (e) {
      setError(
        `Sign-in failed: ${e instanceof Error ? e.message : String(e)}`,
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="page">
      <header className="page-head">
        <h1>Donate spare plan capacity to open source</h1>
        <p className="lede">
          Don-a-Token runs small, sandboxed Codex jobs for projects you pick,
          using ChatGPT plan capacity you weren't going to use anyway.
          Folding@home for tokens.
        </p>
      </header>

      <div className="card">
        <h2>Providers</h2>
        <ul className="provider-list">
          <li>
            <span className="provider-name">ChatGPT</span>
            <span className="provider-note">
              Sign in with ChatGPT, including plan usage
            </span>
          </li>
          <li className="muted">
            <span className="provider-name">More providers</span>
            <span className="provider-note">
              Additional model providers can plug in here later
            </span>
          </li>
        </ul>

        {/* TODO(auth-spike): use the approved OpenAI button assets, see
            https://developers.openai.com/siwc/website */}
        <button
          className="btn btn-primary btn-chatgpt"
          onClick={handleContinue}
          disabled={busy}
        >
          {busy ? "Waiting for browser approval…" : "Continue with ChatGPT"}
        </button>
        {busy && (
          <p className="fineprint">
            Approve the request in the browser window that just opened, then
            return here.
          </p>
        )}
        {error && <p className="error">{error}</p>}

        <p className="fineprint">
          Eligible requests use your ChatGPT plan. Review usage anytime:{" "}
          <a
            href={MANAGE_USAGE_URL}
            onClick={(e) => {
              e.preventDefault();
              void openExternal(MANAGE_USAGE_URL);
            }}
          >
            Manage usage
          </a>
          .
        </p>
      </div>

      <PlanBadge />
      <p>
        <button className="btn btn-ghost" onClick={onExplore}>
          Explore the UI without signing in
        </button>
      </p>
    </section>
  );
}
