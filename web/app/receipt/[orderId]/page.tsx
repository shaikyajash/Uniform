import { Suspense } from "react";
import { Frame } from "@/app/_ui/frame";
import { PayCardSkeleton } from "@/app/pay/[orderId]/parts";
import { Receipt } from "./receipt";

/** `/receipt/<orderId>`: a public record of one order, with links to every transaction. */
export default function ReceiptPage({ params }: PageProps<"/receipt/[orderId]">) {
  return (
    <main className="relative grid min-h-screen justify-items-center content-start overflow-x-hidden px-4 pt-[max(96px,16vh)] pb-12">
      <Frame />
      <Suspense fallback={<PayCardSkeleton />}>
        {params.then(({ orderId }) => (
          <Receipt orderId={orderId} />
        ))}
      </Suspense>
    </main>
  );
}
