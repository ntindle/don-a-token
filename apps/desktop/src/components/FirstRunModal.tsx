interface Props {
  onDismiss: () => void;
}

/** First-sign-in confirmation, shown exactly once per the SIWC guidelines. */
export default function FirstRunModal({ onDismiss }: Props) {
  return (
    <div className="modal-backdrop" role="dialog" aria-modal="true">
      <div className="modal">
        <h2>You&rsquo;re using your ChatGPT plan</h2>
        <p>
          Eligible AI requests in Don-a-Token will use your ChatGPT plan. You
          can review and manage usage in ChatGPT settings at any time.
        </p>
        <button className="btn btn-primary" onClick={onDismiss} autoFocus>
          Got it
        </button>
      </div>
    </div>
  );
}
