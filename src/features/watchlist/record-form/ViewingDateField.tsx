interface Props {
  label: string; value: string; yearOnly: boolean;
  onYearOnlyChange: (value: boolean) => void; onChange: (value: string) => void;
}

export default function ViewingDateField({ label, value, yearOnly, onYearOnlyChange, onChange }: Props) {
  // Switching to full precision retains the known year as an incomplete draft.
  // Browser pattern validation requires the user to supply the actual month/day
  // or explicitly clear it. Merely switching a control never invents a date.
  const incompleteDay = !yearOnly && value.length === 4;
  return <div>
    <label className="block text-sm font-medium text-gray-700">{label}
      <input aria-label={label} type={yearOnly || incompleteDay ? 'text' : 'date'}
        inputMode={yearOnly ? 'numeric' : undefined} value={value}
        pattern={yearOnly ? '[0-9]{4}' : '[0-9]{4}-[0-9]{2}-[0-9]{2}'}
        placeholder={yearOnly ? 'YYYY' : 'YYYY-MM-DD'}
        onChange={e => onChange(e.target.value)}
        className="w-full px-3 py-2 rounded-xl border border-gray-200 text-sm" />
    </label>
    <label className="flex items-center gap-1 text-xs text-gray-500">
      <input type="checkbox" aria-label={`${label}仅年份`} checked={yearOnly} onChange={e => {
        onYearOnlyChange(e.target.checked);
        if (e.target.checked && value.length === 10) onChange(value.slice(0, 4));
      }} />仅选年份
    </label>
    {incompleteDay && <p className="text-xs text-gray-500">请输入已知月日，或清空日期。已知年份仍保留在草稿中。</p>}
  </div>;
}
