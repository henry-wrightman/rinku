interface HeaderProps {
  connected: boolean;
}

export function Header({ connected }: HeaderProps) {
  return (
    <header>
      <h1>rinku explorer</h1>
      <p>rinku: where the link is the ledger</p>
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
