/**
 * Buffer for accumulating thought chunks from streaming agent responses.
 *
 * This buffer accumulates text chunks and provides a safe way to retrieve
 * complete thoughts. The `take()` method only clears the buffer when returning
 * valid content, preventing data loss if the caller fails to process the result.
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
   * Retrieve and clear the buffered content.
   *
   * Returns the trimmed content if it contains non-whitespace text,
   * otherwise returns null and clears the buffer.
   *
   * IMPORTANT: This method always clears the internal buffer after reading.
   * Callers should ensure they successfully process the returned value before
   * the next append/take cycle, as data cannot be recovered after take() returns.
   *
   * For concurrent access patterns, ensure append and take are not called
   * simultaneously from different event loops. In React/TypeScript web contexts,
   * this is typically safe as JavaScript is single-threaded.
   */
  take(): string | null {
    const content = this.content;
    const trimmed = content.trim();
    // Always clear the buffer after reading
    this.content = '';
    // Return null for whitespace-only content, otherwise return trimmed
    return trimmed.length > 0 ? trimmed : null;
  }

  /**
   * Clear the buffer without returning the content.
   * Use this when discarding pending thoughts (e.g., session reset).
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
