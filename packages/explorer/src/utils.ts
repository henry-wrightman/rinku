export const truncate = (s: string, len = 8): string => {
  if (!s || s.length <= len) return s;
  return `${s.slice(0, len)}...`;
};

export const timeAgo = (ts: number): string => {
  const diff = Date.now() - ts;
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return "just now";
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
};

export const formatNumber = (n: number): string => {
  if (n >= 1_000_000_000) {
    return (n / 1_000_000_000).toFixed(1).replace(/\.0$/, "") + "B";
  }
  if (n >= 1_000_000) {
    return (n / 1_000_000).toFixed(1).replace(/\.0$/, "") + "M";
  }
  if (n >= 1_000) {
    return (n / 1_000).toFixed(1).replace(/\.0$/, "") + "k";
  }
  return n?.toFixed(2).toString();
};

export const formatTps = (tps: number): string => {
  if (tps < 0.01) return "0";
  if (tps < 1) return tps.toFixed(2);
  if (tps < 10) return tps.toFixed(1);
  return Math.round(tps).toString();
};
