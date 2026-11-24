/** The message of whatever was thrown, or `fallback`. */
export const messageOf = (err: unknown, fallback: string): string =>
  typeof err === "string" ? err || fallback : (err as { message?: string } | undefined)?.message || fallback;
