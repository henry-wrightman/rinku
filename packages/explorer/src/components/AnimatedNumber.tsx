import * as React from "react";
import { AnimatePresence, motion, useReducedMotion } from "framer-motion";

interface AnimatedNumberProps {
  value: number;
  formatShort: (n: number) => string;
  formatLong?: (n: number) => string;
  className?: string;
  titleWhenExpandable?: boolean;
}

function formatNumberLong(n: number): string {
  return n.toLocaleString();
}

function isShorthand(short: string, long: string): boolean {
  const s = short.toLowerCase();
  return short !== long && (/[kmb]/.test(s) || short.length < long.length);
}

function scaleForLongText(longValue: string) {
  // Smooth-ish shrink as strings get very long
  const base = 1;
  const over = Math.max(0, longValue.length - 8);
  return Math.max(0.72, base - over * 0.035);
}

function AnimatedChars({ text, reduced }: { text: string; reduced: boolean }) {
  const chars = React.useMemo(() => Array.from(text), [text]);

  if (reduced) {
    return <span style={{ whiteSpace: "nowrap" }}>{text}</span>;
  }

  return (
    <span style={{ display: "inline-block", whiteSpace: "nowrap" }}>
      {chars.map((ch, i) => (
        <motion.span
          key={`${text}-${i}`}
          initial={{ y: 10, opacity: 0, filter: "blur(6px)" }}
          animate={{ y: 0, opacity: 1, filter: "blur(0px)" }}
          exit={{ y: -8, opacity: 0, filter: "blur(6px)" }}
          transition={{
            duration: 0.22,
            ease: "easeOut",
            delay: i * 0.014,
          }}
          style={{ display: "inline-block" }}
        >
          {ch === " " ? "\u00A0" : ch}
        </motion.span>
      ))}
    </span>
  );
}

export function AnimatedNumber({
  value,
  formatShort,
  formatLong = formatNumberLong,
  className,
  titleWhenExpandable = true,
}: AnimatedNumberProps) {
  const prefersReduced = useReducedMotion();
  const [expanded, setExpanded] = React.useState(false);

  const shortValue = React.useMemo(
    () => formatShort(value),
    [value, formatShort],
  );
  const longValue = React.useMemo(() => formatLong(value), [value, formatLong]);

  const hasShorthand = React.useMemo(
    () => isShorthand(shortValue, longValue),
    [shortValue, longValue],
  );

  const showLong = expanded && hasShorthand;
  const longScale = React.useMemo(
    () => scaleForLongText(longValue),
    [longValue],
  );

  return (
    <motion.span
      className={["cool-animated-number", className].filter(Boolean).join(" ")}
      data-expanded={showLong ? "true" : "false"}
      layout
      onPointerEnter={() => hasShorthand && setExpanded(true)}
      onPointerLeave={() => setExpanded(false)}
      onFocus={() => hasShorthand && setExpanded(true)}
      onBlur={() => setExpanded(false)}
      tabIndex={hasShorthand ? 0 : -1}
      title={titleWhenExpandable && hasShorthand ? longValue : undefined}
      style={{
        display: "inline-flex",
        alignItems: "baseline",
        cursor: hasShorthand ? "pointer" : "default",
        userSelect: "none",
        whiteSpace: "nowrap",
      }}
      transition={
        prefersReduced
          ? { duration: 0 }
          : {
              type: "spring",
              stiffness: 520,
              damping: 32,
              mass: 0.6,
            }
      }
    >
      {/* Background “pill” that grows/collapses */}
      <motion.span
        aria-hidden="true"
        layout
        style={{
          position: "absolute",
          inset: 0,
          borderRadius: 999,
          pointerEvents: "none",
        }}
      />

      <motion.span
        layout
        style={{
          position: "relative",
          borderRadius: 999,
          padding: showLong ? "6px 10px" : "4px 8px",
          lineHeight: 1,
          fontVariantNumeric: "tabular-nums",
        }}
        animate={
          prefersReduced
            ? {}
            : {
                scale: showLong ? 1 : 1,
              }
        }
      >
        <AnimatePresence mode="wait" initial={false}>
          {showLong ? (
            <motion.span
              key="long"
              layout
              initial={prefersReduced ? false : { opacity: 0, y: 8 }}
              animate={prefersReduced ? {} : { opacity: 1, y: 0 }}
              exit={prefersReduced ? {} : { opacity: 0, y: -6 }}
              transition={{ duration: 0.18, ease: "easeOut" }}
              style={{
                display: "inline-block",
                transformOrigin: "left bottom",
              }}
            >
              <motion.span
                animate={prefersReduced ? {} : { scale: longScale }}
                transition={
                  prefersReduced
                    ? { duration: 0 }
                    : { type: "spring", stiffness: 420, damping: 30 }
                }
                style={{ display: "inline-block" }}
              >
                <AnimatedChars text={longValue} reduced={prefersReduced} />
              </motion.span>
            </motion.span>
          ) : (
            <motion.span
              key="short"
              layout
              initial={prefersReduced ? false : { opacity: 0, y: 8 }}
              animate={prefersReduced ? {} : { opacity: 1, y: 0 }}
              exit={prefersReduced ? {} : { opacity: 0, y: -6 }}
              transition={{ duration: 0.18, ease: "easeOut" }}
              style={{ display: "inline-block" }}
            >
              <AnimatedChars text={shortValue} reduced={prefersReduced} />
            </motion.span>
          )}
        </AnimatePresence>
      </motion.span>
    </motion.span>
  );
}
