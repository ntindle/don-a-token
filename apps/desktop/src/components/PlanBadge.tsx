import { MANAGE_USAGE_URL, openExternal } from "../lib/tauri";

/** Persistent "Using ChatGPT plan" indicator per the SIWC UI/UX guidelines. */
export default function PlanBadge() {
  return (
    <p className="plan-badge">
      <span className="plan-dot" aria-hidden="true" />
      Using ChatGPT plan ·{" "}
      <a
        href={MANAGE_USAGE_URL}
        onClick={(e) => {
          e.preventDefault();
          void openExternal(MANAGE_USAGE_URL);
        }}
      >
        Manage usage
      </a>
    </p>
  );
}
