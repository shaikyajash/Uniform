import { Suspense } from "react";
import { PayCard } from "./pay-card";
import styles from "./pay.module.css";

/** `/pay/<orderId>`: the link the Order Desk gives the shop as `payUrl`. */
export default function PayPage({ params }: PageProps<"/pay/[orderId]">) {
  return (
    <main className={styles.page}>
      <Suspense fallback={<section className={styles.card} aria-busy="true" />}>
        {params.then(({ orderId }) => (
          <PayCard orderId={orderId} />
        ))}
      </Suspense>
    </main>
  );
}
