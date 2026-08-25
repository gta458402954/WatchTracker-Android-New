import assert from 'node:assert/strict';
import test from 'node:test';
import { detectRuntime } from '../runtime.ts';
import { androidBackActionForRoute, androidBackResultForBridge, navigationMode, recordIdFromHash, routeFromHash } from '../navigation.ts';

test('Android runtime requires both Tauri and Android user agent', () => {
  assert.equal(detectRuntime({ hasTauri: true, userAgent: 'Mozilla/5.0 Android 16; Pixel' }), 'android');
  assert.equal(detectRuntime({ hasTauri: true, userAgent: 'Mozilla/5.0 Windows NT 10.0' }), 'tauri-desktop');
  assert.equal(detectRuntime({ hasTauri: false, userAgent: 'Mozilla/5.0 Android 16; Pixel' }), 'web');
});

test('mobile navigation maps supported hashes and falls back to library', () => {
  assert.equal(routeFromHash('#settings'), 'settings');
  assert.equal(routeFromHash('form'), 'form');
  assert.equal(routeFromHash('#unknown'), 'library');
  assert.equal(routeFromHash(''), 'library');
});

test('mobile navigation restores ids from detail and edit hashes across refresh/forward', () => {
  assert.equal(recordIdFromHash('#detail/a%2Fb'), 'a/b');
  assert.equal(recordIdFromHash('#form/edit/a%2Fb'), 'a/b');
  assert.equal(recordIdFromHash('#form/new'), null);
});

test('tabs are internal state and never create browser history entries', () => {
  assert.equal(navigationMode('settings', 'library'), 'internal');
  assert.equal(navigationMode('library', 'settings'), 'internal');
  assert.equal(navigationMode('library', 'form'), 'internal');
});

test('mobile root back exits while form and placeholders traverse history', () => {
  assert.equal(androidBackActionForRoute('library'), 'exit');
  assert.equal(androidBackActionForRoute('settings'), 'history');
  assert.equal(androidBackActionForRoute('form'), 'history');
});

test('Android bridge contract preserves consumed overlays and only finishes at root', () => {
  assert.equal(androidBackResultForBridge('consumed', false), 'consumed');
  assert.equal(androidBackResultForBridge('history', true), 'history');
  assert.equal(androidBackResultForBridge('history', false), 'finish');
  assert.equal(androidBackResultForBridge('exit', true), 'finish');
});
