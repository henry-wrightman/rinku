interface HeaderProps {
  connected: boolean;
}

export function Header({ connected }: HeaderProps) {
  return (
    <header>
      <h1>rinku explorer</h1>
      <p>
        rinku: a url-native distributed ledger, where links are the data and the
        proof.
      </p>
      <div className="status-indicator">
        <span
          className={`status-dot ${connected ? "connected" : "disconnected"}`}
        ></span>
        <span
          className={`status-text ${connected ? "connected" : "disconnected"}`}
        >
          {connected ? "connected" : "disconnected"}
        </span>
        <span className={`status-text`}>[testnet]</span>
      </div>
    </header>
  );
}
