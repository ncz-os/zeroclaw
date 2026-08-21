/**
 * Buffer for accumulating thought chunks from streaming agent responses.
 *
 * This buffer accumulates text chunks and provides a safe way to retrieve
 * complete thoughts. The `take()` method returns the content and immediately
 * clears the buffer, preventing interleaving when async operations are pending.
 * Callers should process the returned content and handle any errors appropriately.
 *
 * Thread Safety:
 * - All methods are thread-safe using internal mutex protection
 * - `take()` atomically retrieves and clears the buffer
 * - `append()` is safe to call concurrently with `take()`
 * - No explicit locking required by callers - the buffer manages its own synchronization
 */
export class ThoughtChunkBuffer {
  private content = '';
  private readonly mutex = new Mutex();

  /**
   * Append a chunk of thought text to the buffer.
   *
   * Thread-safe: uses internal mutex to prevent interleaving.
   */
  async append(chunk: string): Promise<void> {
    const guard = await this.mutex.lock();
    try {
      this.content += chunk;
    } finally {
      guard.unlock();
    }
  }

  /**
   * Retrieve the buffered content and immediately clear the buffer.
   *
   * Returns the trimmed content if it contains non-whitespace text,
   * otherwise returns null. The buffer is atomically taken and cleared
   * to prevent concurrent modifications.
   *
   * Thread-safe: uses internal mutex for atomic take-and-clear.
   */
  async take(): Promise<string | null> {
    const guard = await this.mutex.lock();
    try {
      const trimmed = this.content.trim();
      this.content = '';
      return trimmed.length > 0 ? trimmed : null;
    } finally {
      guard.unlock();
    }
  }

  /**
   * Clear the buffer without taking content.
   *
   * Thread-safe: uses internal mutex.
   *
   * Use this when discarding pending thoughts (e.g., session reset).
   */
  async clear(): Promise<void> {
    const guard = await this.mutex.lock();
    try {
      this.content = '';
    } finally {
      guard.unlock();
    }
  }

  /**
   * Check if the buffer has any content (for debugging/testing).
   * Thread-safe.
   */
  async isEmpty(): Promise<boolean> {
    const guard = await this.mutex.lock();
    try {
      return this.content.length === 0;
    } finally {
      guard.unlock();
    }
  }
}

/**
 * Simple async mutex implementation for protecting shared state.
 */
class Mutex {
  private locked = false;
  private waiters: Array<() => void> = [];

  async lock(): Promise<MutexGuard> {
    if (!this.locked) {
      this.locked = true;
      return new MutexGuard(this);
    }

    return new Promise<MutexGuard>((resolve) => {
      this.waiters.push(() => {
        this.locked = true;
        resolve(new MutexGuard(this));
      });
    });
  }

  unlock(): void {
    if (this.waiters.length > 0) {
      const next = this.waiters.shift()!;
      next();
    } else {
      this.locked = false;
    }
  }
}

class MutexGuard {
  private mutex: Mutex;

  constructor(mutex: Mutex) {
    this.mutex = mutex;
  }

  unlock(): void {
    this.mutex.unlock();
  }
}
