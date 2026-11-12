import React from "react";

export interface Props {
  label: string;
}

export function Button({ label }: Props) {
  return <button>{label}</button>;
}

export const App = () => <Button label="hi" />;
