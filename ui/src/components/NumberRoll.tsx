const decimal = (value: number) => value.toFixed(1);

export function NumberRoll({ value, format = decimal, className }: {
  value: number;
  format?: (value: number) => string;
  className?: string;
}) {
  const text = format(value);
  return <span className={className}><span className="number-value" key={text}>{text}</span></span>;
}
