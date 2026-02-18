interface HeaderProps {
  connected: boolean;
  protocolVersion?: string;
  nodeVersion?: string;
  peersConnected?: number;
}

export function Header({
  connected,
  protocolVersion,
  nodeVersion,
  peersConnected,
}: HeaderProps) {
  const COLOR_PALETTES = [
    [
      "#ff6b6b",
      "#ffa500",
      "#ffd93d",
      "#6bcb77",
      "#4d96ff",
      "#9b59b6",
      "#ff6b9d",
    ],
    ["#ff0080", "#ff00ff", "#8000ff", "#0080ff", "#00ffff", "#00ff80"],
    ["#e74c3c", "#e67e22", "#f1c40f", "#2ecc71", "#3498db", "#9b59b6"],
    ["#ff6384", "#36a2eb", "#ffce56", "#4bc0c0", "#9966ff", "#ff9f40"],
    ["#00d9ff", "#00ff88", "#ffff00", "#ff00ff", "#ff3300"],
    ["#ff1493", "#00bfff", "#7fff00", "#ffa500", "#ff69b4", "#00ced1"],
  ];

  type AnimStyle = {
    name: string;
    getStyle: (i: number, colors: string[]) => React.CSSProperties;
  };

  const ANIMATION_STYLES: AnimStyle[] = [
    {
      name: "wave",
      getStyle: (i, colors) => ({
        animation: "rainbow 2s linear infinite",
        animationDelay: `${i * 0.1}s`,
      }),
    },
    {
      name: "shimmer",
      getStyle: (i, colors) => ({
        color: colors[i % colors.length],
        animation: "shimmer 2s ease-in-out infinite",
        animationDelay: `${i * 0.08}s`,
      }),
    },
    {
      name: "pulse",
      getStyle: (i, colors) => ({
        color: colors[i % colors.length],
        animation: "pulse-opacity 1.5s ease-in-out infinite",
        animationDelay: `${i * 0.15}s`,
      }),
    },
    {
      name: "blink-random",
      getStyle: (i, colors) => ({
        animation: `blink-color 0.8s step-end infinite`,
        animationDelay: `${i * 0.1}s`,
      }),
    },
    {
      name: "reverse-wave",
      getStyle: (i, colors) => ({
        animation: "rainbow 2s linear infinite reverse",
        animationDelay: `${i * 0.12}s`,
      }),
    },
  ];

  const randomPalette =
    COLOR_PALETTES[Math.floor(Math.random() * COLOR_PALETTES.length)];
  const randomStyle = ANIMATION_STYLES[3]; // Math.floor(Math.random() * ANIMATION_STYLES.length)

  return (
    <header>
      {/* <h1>rinku explorer</h1> */}
      <p>
        {" "}
        <span style={{}}>
          {"rinku".split("").map((char, i) => (
            <span key={i} style={randomStyle.getStyle(i, randomPalette)}>
              {char === " " ? "\u00A0" : char}
            </span>
          ))}
        </span>
        : where the link is the proof
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
        {protocolVersion && (
          <span className="status-text" title={`Node: v${nodeVersion || "?"}`}>
            v{protocolVersion}
          </span>
        )}
        {peersConnected !== undefined && (
          <span className="status-text" title="Connected peers">
            ({peersConnected + 1} peers)
          </span>
        )}
      </div>
    </header>
  );
}
