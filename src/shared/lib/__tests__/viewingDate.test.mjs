import assert from 'node:assert/strict';
import test from 'node:test';
import { compareViewingDatesDesc, exactViewingDay, viewingDatePrecision, viewingDateText, validViewingDateDraft } from '../viewingDate.ts';
import { seasonRecordMetadata } from '../../../features/collections/lib/tmdbRecordMapping.ts';
import { initialRecordFormValues } from '../../../features/watchlist/record-form/recordFormModel.ts';

test('strict viewing dates distinguish year, actual day and absent date', () => {
  for (const v of ['0001', '2020', '9999']) assert.equal(viewingDatePrecision(v), 'year');
  for (const v of ['2020-02-29', '0001-01-01', '2000-02-29']) assert.equal(viewingDatePrecision(v), 'day');
  for (const v of ['0000', '2021-02-29', '1900-02-29', '２０２０', '2020 ', '2020-00-01', '2020-04-31', 'junk']) {
    assert.equal(viewingDatePrecision(v), null); assert.equal(validViewingDateDraft(v), false);
  }
  assert.equal(validViewingDateDraft(null), true); assert.equal(validViewingDateDraft(''), true);
  assert.equal(exactViewingDay('2020'), null); assert.equal(exactViewingDay(null), null);
  assert.equal(viewingDateText('2020', 'end'), '2020 年看完');
  assert.equal(viewingDateText('2020-05-16', 'start'), '2020 年 5 月 16 日开始观看');
});

test('consistent date sorting never assigns a day to a year', () => {
  const dates = [null, '2020', '2020-01-01', '2021', '2020-12-31'];
  assert.deepEqual(dates.sort(compareViewingDatesDesc), ['2021', '2020-12-31', '2020-01-01', '2020', null]);
});

test('Season 0 is preserved, missing season stays null, ordinary form edit retains precision', () => {
  const series = { id: 4376, name: '执法悍将', original_name: 'JAG' };
  const special = seasonRecordMetadata(series, { id: 12953, season_number: 0, name: '特别篇' });
  assert.equal(special.tmdbSeasonNumber, 0); assert.equal(special.tmdbId, 12953); assert.equal(special.tmdbParentId, 4376);
  assert.equal(seasonRecordMetadata(series, { id: 12954 }).tmdbSeasonNumber, null);
  assert.equal(seasonRecordMetadata(series, { id: 12954, season_number: 1 }).tmdbSeasonNumber, 1);
  const form = initialRecordFormValues({ ...special, startDate: '2020', endDate: '2022', notes: 'before' });
  const edited = { ...form, notes: 'after' };
  assert.equal(edited.startDate, '2020'); assert.equal(edited.endDate, '2022'); assert.equal(edited.tmdbSeasonNumber, 0);
});
