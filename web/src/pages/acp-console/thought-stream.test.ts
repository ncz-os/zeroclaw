import assert from 'node:assert/strict';
import test from 'node:test';

import { ThoughtChunkBuffer } from './thought-stream.ts';

test('consecutive thought chunks accumulate until explicitly cleared', () => {
  const buffer = new ThoughtChunkBuffer();

  buffer.append('I');
  buffer.append(' need');
  buffer.append(' more context.');

  // take() returns content but does NOT clear
  assert.equal(buffer.take(), 'I need more context.');
  // Buffer still has content until clear() is called
  assert.equal(buffer.take(), 'I need more context.');
  
  // Explicitly clear after successful processing
  buffer.clear();
  assert.equal(buffer.take(), null);
});

test('a boundary separates consecutive thought paragraphs', () => {
  const buffer = new ThoughtChunkBuffer();

  buffer.append('First');
  buffer.append(' thought.');
  assert.equal(buffer.take(), 'First thought.');
  
  // Clear after processing
  buffer.clear();

  buffer.append('Second thought.');
  assert.equal(buffer.take(), 'Second thought.');
});

test('draining drops whitespace-only chunks without leaking them forward', () => {
  const buffer = new ThoughtChunkBuffer();

  buffer.append('  \n');
  assert.equal(buffer.take(), null);

  buffer.append('Visible');
  assert.equal(buffer.take(), 'Visible');
  
  // Clear after processing
  buffer.clear();
});

test('clearing discards a pending thought when the session resets', () => {
  const buffer = new ThoughtChunkBuffer();

  buffer.append('stale session thought');
  buffer.clear();

  assert.equal(buffer.take(), null);
});

test('take() does not clear buffer - caller must clear after success', () => {
  const buffer = new ThoughtChunkBuffer();

  buffer.append('test content');
  
  // First take returns content
  const first = buffer.take();
  assert.equal(first, 'test content');
  
  // Buffer not cleared - second take returns same content
  const second = buffer.take();
  assert.equal(second, 'test content');
  
  // Caller clears after successful processing
  buffer.clear();
  assert.equal(buffer.take(), null);
});

test('isEmpty() reflects current buffer state', () => {
  const buffer = new ThoughtChunkBuffer();
  
  assert.equal(buffer.isEmpty(), true);
  
  buffer.append('test');
  assert.equal(buffer.isEmpty(), false);
  
  buffer.clear();
  assert.equal(buffer.isEmpty(), true);
});
