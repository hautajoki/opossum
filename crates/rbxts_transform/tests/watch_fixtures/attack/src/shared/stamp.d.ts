/**
 * The call becomes "file:line:column".
 * @stamp
 */
declare function here(): string;

/**
 * A build-wide id, numbered in program order at finish.
 * @stamp build
 */
declare function buildId(): string;

/** @stamp mystery */
declare function mystery(): string;
