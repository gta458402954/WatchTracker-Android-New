import type { RecordFormValues } from './useTmdbRecordSearch';
interface RecordDetailsFieldsProps { form: RecordFormValues; startYearOnly: boolean; endYearOnly: boolean; years: number[]; onStartYearOnlyChange: (value: boolean) => void; onEndYearOnlyChange: (value: boolean) => void; onInterestLevelChange: (value: number | null) => void; onRatingChange: (value: number | null) => void; onStartDateChange: (value: string) => void; onEndDateChange: (value: string) => void; onNotesChange: (value: string) => void; }
export default function RecordDetailsFields({ form, startYearOnly, endYearOnly, years, onStartYearOnlyChange, onEndYearOnlyChange, onInterestLevelChange, onRatingChange, onStartDateChange, onEndDateChange, onNotesChange }: RecordDetailsFieldsProps) {
  return <div>          {/* Rating / Interest Level */}
          {form.status === '未看' ? (
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">期待值 (Watch Value)</label>
              <div className="flex gap-2 items-center">
                {[1, 2, 3, 4, 5].map(star => (
                  <button
                    key={star}
                    type="button"
                    onClick={() => onInterestLevelChange(form.interestLevel === star ? null : star)}
                    className={`text-2xl transition-transform hover:scale-110 ${
                      form.interestLevel != null && star <= (form.interestLevel ?? 0)
                        ? 'text-rose-400'
                        : 'text-gray-200 hover:text-rose-300'
                    }`}
                  >
                    ❤
                  </button>
                ))}
                {form.interestLevel != null && (
                  <span className="text-sm text-gray-400 ml-1">
                    {['', '随便看看', '有点兴趣', '值得一看', '非常期待', '必看神作'][form.interestLevel ?? 0]}
                  </span>
                )}
              </div>
            </div>
          ) : (
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">评分</label>
              <div className="flex gap-1.5 items-center flex-wrap">
                {[1, 2, 3, 4, 5, 6, 7, 8, 9, 10].map(star => (
                  <button
                    key={star}
                    type="button"
                    onClick={() => onRatingChange(form.rating === star ? null : star)}
                    className={`text-xl transition-transform hover:scale-110 ${
                      form.rating != null && star <= (form.rating ?? 0)
                        ? 'text-amber-400'
                        : 'text-gray-200 hover:text-amber-300'
                    }`}
                  >
                    ★
                  </button>
                ))}
                {form.rating != null && (
                  <span className="text-xs text-gray-500 ml-1.5 font-medium bg-gray-50 border border-gray-150 px-2 py-0.5 rounded-md">
                    {['', '很差 (1/10)', '差 (2/10)', '较差 (3/10)', '一般 (4/10)', '还行 (5/10)', '较好 (6/10)', '好 (7/10)', '很好 (8/10)', '超棒 (9/10)', '神作 (10/10)'][form.rating ?? 0]}
                  </span>
                )}
              </div>
            </div>
          )}

          {/* Dates */}
          <div className="grid grid-cols-2 gap-3">
            <div>
              <div className="flex items-center justify-between mb-1">
                <label className="text-sm font-medium text-gray-700">开始时间</label>
                <label className="flex items-center gap-1 text-[11px] text-gray-400 hover:text-gray-600 cursor-pointer select-none">
                  <input
                    type="checkbox"
                    checked={startYearOnly}
                    onChange={(e) => {
                      const checked = e.target.checked;
                      onStartYearOnlyChange(checked);
                      if (checked) {
                        const yr = form.startDate ? form.startDate.slice(0, 4) : new Date().getFullYear().toString();
                        onStartDateChange(yr);
                      } else {
                        const dateVal = form.startDate && /^\d{4}$/.test(form.startDate) ? `${form.startDate}-01-01` : '';
                        onStartDateChange(dateVal);
                      }
                    }}
                    className="rounded border-gray-300 text-indigo-600 focus:ring-indigo-500 w-3 h-3"
                  />
                  仅选年份
                </label>
              </div>
              {startYearOnly ? (
                <select
                  value={form.startDate || ''}
                  onChange={e => onStartDateChange(e.target.value)}
                  className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition bg-white"
                >
                  <option value="">选择年份</option>
                  {years.map(y => (
                    <option key={y} value={y.toString()}>{y}年</option>
                  ))}
                </select>
              ) : (
                <input
                  type="date"
                  value={form.startDate}
                  onChange={e => onStartDateChange(e.target.value)}
                  className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition"
                />
              )}
            </div>
            <div>
              <div className="flex items-center justify-between mb-1">
                <label className="text-sm font-medium text-gray-700">结束时间</label>
                <label className="flex items-center gap-1 text-[11px] text-gray-400 hover:text-gray-600 cursor-pointer select-none">
                  <input
                    type="checkbox"
                    checked={endYearOnly}
                    onChange={(e) => {
                      const checked = e.target.checked;
                      onEndYearOnlyChange(checked);
                      if (checked) {
                        const yr = form.endDate ? form.endDate.slice(0, 4) : new Date().getFullYear().toString();
                        onEndDateChange(yr);
                      } else {
                        const dateVal = form.endDate && /^\d{4}$/.test(form.endDate) ? `${form.endDate}-01-01` : '';
                        onEndDateChange(dateVal);
                      }
                    }}
                    className="rounded border-gray-300 text-indigo-600 focus:ring-indigo-500 w-3 h-3"
                  />
                  仅选年份
                </label>
              </div>
              {endYearOnly ? (
                <select
                  value={form.endDate || ''}
                  onChange={e => onEndDateChange(e.target.value)}
                  className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition bg-white"
                >
                  <option value="">选择年份</option>
                  {years.map(y => (
                    <option key={y} value={y.toString()}>{y}年</option>
                  ))}
                </select>
              ) : (
                <input
                  type="date"
                  value={form.endDate}
                  onChange={e => onEndDateChange(e.target.value)}
                  className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition"
                />
              )}
            </div>
          </div>

          {/* Notes */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-1">备注</label>
            <textarea
              value={form.notes}
              onChange={e => onNotesChange(e.target.value)}
              placeholder="随便写点什么..."
              rows={2}
              className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm outline-none focus:border-indigo-400 focus:ring-2 focus:ring-indigo-100 transition resize-none"
            />
          </div>


  </div>;
}
