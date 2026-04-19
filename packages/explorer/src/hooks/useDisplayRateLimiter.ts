import { useEffect, useRef, useState } from "react";

export interface RateLimiterOptions {
  targetCadenceMs?: number;
  maxVisible?: number;
  maxQueue?: number;
}

export interface RateLimiterResult<T> {
  visibleNodes: T[];
  overflowPerSec: number;
  arrivalsPerSec: number;
  displayedPerSec: number;
  queueDepth: number;
}

export function useDisplayRateLimiter<T extends { hash: string }>(
  incomingNodes: T[],
  options: RateLimiterOptions = {},
): RateLimiterResult<T> {
  const {
    targetCadenceMs = 100,
    maxVisible = 12,
    maxQueue = 500,
  } = options;

  const [visibleNodes, setVisibleNodes] = useState<T[]>([]);
  const [stats, setStats] = useState({
    overflowPerSec: 0,
    arrivalsPerSec: 0,
    displayedPerSec: 0,
    queueDepth: 0,
  });

  const queueRef = useRef<T[]>([]);
  const seenHashesRef = useRef<Set<string>>(new Set());
  const arrivalsBucketRef = useRef<number>(0);
  const displayedBucketRef = useRef<number>(0);
  const droppedBucketRef = useRef<number>(0);
  const lastTickRef = useRef<number>(performance.now());
  const lastStatsTickRef = useRef<number>(performance.now());
  const rafRef = useRef<number | null>(null);

  useEffect(() => {
    if (!incomingNodes || incomingNodes.length === 0) return;

    let added = 0;
    for (const node of incomingNodes) {
      if (!node || !node.hash) continue;
      if (seenHashesRef.current.has(node.hash)) continue;
      seenHashesRef.current.add(node.hash);
      queueRef.current.push(node);
      added++;
    }
    arrivalsBucketRef.current += added;

    if (queueRef.current.length > maxQueue) {
      const drop = queueRef.current.length - maxQueue;
      queueRef.current.splice(0, drop);
      droppedBucketRef.current += drop;
    }

    if (seenHashesRef.current.size > 5000) {
      const arr = Array.from(seenHashesRef.current);
      seenHashesRef.current = new Set(arr.slice(-2500));
    }
  }, [incomingNodes, maxQueue]);

  useEffect(() => {
    let cancelled = false;

    const tick = (now: number) => {
      if (cancelled) return;

      const elapsed = now - lastTickRef.current;
      if (elapsed >= targetCadenceMs && queueRef.current.length > 0) {
        const releaseCount = Math.min(
          Math.max(1, Math.floor(elapsed / targetCadenceMs)),
          queueRef.current.length,
          4,
        );
        const released = queueRef.current.splice(0, releaseCount);
        displayedBucketRef.current += released.length;
        lastTickRef.current = now;

        setVisibleNodes((prev) => {
          const next = [...released.reverse(), ...prev];
          if (next.length > maxVisible) {
            next.length = maxVisible;
          }
          return next;
        });
      }

      const statsElapsed = now - lastStatsTickRef.current;
      if (statsElapsed >= 1000) {
        const scale = 1000 / statsElapsed;
        const arrivals = arrivalsBucketRef.current * scale;
        const displayed = displayedBucketRef.current * scale;
        const dropped = droppedBucketRef.current * scale;
        const overflow = Math.max(0, arrivals - displayed) + dropped;

        setStats({
          arrivalsPerSec: Math.round(arrivals),
          displayedPerSec: Math.round(displayed),
          overflowPerSec: Math.round(overflow),
          queueDepth: queueRef.current.length,
        });

        arrivalsBucketRef.current = 0;
        displayedBucketRef.current = 0;
        droppedBucketRef.current = 0;
        lastStatsTickRef.current = now;
      }

      rafRef.current = requestAnimationFrame(tick);
    };

    rafRef.current = requestAnimationFrame(tick);
    return () => {
      cancelled = true;
      if (rafRef.current !== null) cancelAnimationFrame(rafRef.current);
    };
  }, [targetCadenceMs, maxVisible]);

  return {
    visibleNodes,
    overflowPerSec: stats.overflowPerSec,
    arrivalsPerSec: stats.arrivalsPerSec,
    displayedPerSec: stats.displayedPerSec,
    queueDepth: stats.queueDepth,
  };
}
