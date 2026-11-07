// The receipt as a PDF file the merchant or customer can keep: the same lines as the receipt card,
// in black on white, every transaction a link to its explorer. jsPDF loads only on a download.

import type { Order } from "@/lib/desk";
import {
  STATUS_TEXT,
  formatTime,
  receiptAmount,
  receiptRoute,
  receiptTime,
  receiptTransactions,
} from "./model";

const PAGE_WIDTH = 210; // A4, millimetres
const MARGIN = 22;
const RIGHT = PAGE_WIDTH - MARGIN;
const INK = 17;
const MUTED = 110;
const HAIR = 222;

export async function downloadReceipt(order: Order): Promise<void> {
  const { jsPDF } = await import("jspdf");
  const doc = new jsPDF({ unit: "mm", format: "a4" });
  // The PDF's built-in fonts have no arrow; everything else on a receipt is plain Latin.
  const plain = (text: string) => text.replace(/→/g, "->");
  let y = MARGIN;

  const label = (
    text: string,
    x: number,
    at: number,
    align: "left" | "right" = "left",
  ) => {
    doc.setFont("courier", "normal");
    doc.setFontSize(8);
    doc.setTextColor(MUTED);
    doc.text(text.toUpperCase(), x, at, { align, charSpace: 0.4 });
  };
  const rule = (at: number) => {
    doc.setDrawColor(HAIR);
    doc.setLineWidth(0.2);
    doc.line(MARGIN, at, RIGHT, at);
  };

  label("Uniform · Receipt", MARGIN, y);
  label(`Order ${order.reference}`, RIGHT, y, "right");
  y += 5;
  rule(y);

  y += 14;
  doc.setFont("helvetica", "normal");
  doc.setFontSize(22);
  doc.setTextColor(INK);
  doc.text(order.merchant.name, MARGIN, y);
  y += 7;
  doc.setFontSize(10.5);
  doc.setTextColor(MUTED);
  doc.text(STATUS_TEXT[order.status], MARGIN, y);

  y += 20;
  const amount = receiptAmount(order);
  doc.setFontSize(38);
  doc.setTextColor(INK);
  doc.text(amount, MARGIN, y);
  const amountWidth = doc.getTextWidth(amount);
  doc.setFont("courier", "normal");
  doc.setFontSize(11);
  doc.setTextColor(MUTED);
  doc.text("USDT", MARGIN + amountWidth + 3, y);

  y += 12;
  const time = receiptTime(order);
  const rows: [string, string][] = [
    ["Paid to", order.merchant.address],
    ...(order.payer ? ([["Paid by", order.payer]] as [string, string][]) : []),
    ["Route", plain(receiptRoute(order))],
    [time.label, formatTime(time.unix)],
    ["Order id", order.orderId],
  ];
  rule(y);
  for (const [name, value] of rows) {
    y += 8;
    label(name, MARGIN, y);
    const long = value.startsWith("0x");
    doc.setFont(long ? "courier" : "helvetica", "normal");
    doc.setFontSize(long ? 8.5 : 10);
    doc.setTextColor(INK);
    doc.text(value, RIGHT, y, { align: "right" });
    y += 4;
    rule(y);
  }

  const transactions = receiptTransactions(order);
  if (transactions.length > 0) {
    y += 14;
    label("Transactions", MARGIN, y);
    for (const line of transactions) {
      y += 8;
      doc.setFont("helvetica", "normal");
      doc.setFontSize(10);
      doc.setTextColor(INK);
      doc.text(plain(line.label), MARGIN, y);
      doc.setFont("courier", "normal");
      doc.setFontSize(8);
      doc.setTextColor(MUTED);
      doc.textWithLink(
        "View on explorer",
        RIGHT - doc.getTextWidth("View on explorer"),
        y,
        { url: line.href },
      );
      y += 4;
      rule(y);
    }
  }

  const footer = 297 - MARGIN;
  label("Every line above can be checked on the chain", MARGIN, footer);
  doc.setFont("courier", "normal");
  doc.setFontSize(8);
  doc.setTextColor(MUTED);
  const link = `${window.location.origin}/receipt/${order.orderId}`;
  doc.textWithLink(
    "Open online",
    RIGHT - doc.getTextWidth("Open online"),
    footer,
    { url: link },
  );

  doc.save(`uniform-receipt-${order.reference.replace(/[^\w.-]+/g, "-")}.pdf`);
}
