/**
 * Buffer for accumulating thought chunks from streaming agent responses.
 *
 * This buffer accumulates text chunks and provides a safe way to retrieve
 * complete thoughts. The `take()` method returns the content without clearing
 * the buffer, allowing the caller to clear it only after successful processing.
 */
export class ThoughtChunkBuffer {
  private content = '';

  /**
   * Append a chunk of thought text to the buffer.
   */
  append(chunk: string): void {
    this.content += chunk;
  }

  /**
   * Retrieve the buffered content without clearing it.
   *
   * Returns the trimmed content if it contains non-whitespace text,
   * otherwise returns null. The buffer is NOT cleared — callers must
   * explicitly call `clear()` after successfully processing the result.
   *
   * This design prevents data loss if the caller's processing logic fails
   * or throws an exception after retrieving the content.
   */
  take(): string | null {
    const trimmed = this.content.trim();
    return trimmed.length > 0 ? trimmed : null;
  }

  /**
   * Clear the buffer.
   *
   * Use this after successfully processing content from `take()`, or when
   * discarding pending thoughts (e.g., session reset).
   */
  clear(): void {
    this.content = '';
  }

  /**
   * Check if the buffer has any content (for debugging/testing).
   */
  isEmpty(): boolean {
    return this.content.length === 0;
  }
}
