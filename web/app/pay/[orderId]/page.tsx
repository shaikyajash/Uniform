import { Suspense } from "react";
import { Frame } from "@/app/_ui/frame";
import { PayCard, PayCardSkeleton } from "./pay-card";

/** `/pay/<orderId>`: the link the Order Desk gives the shop as `payUrl`. */
export default function PayPage({ params }: PageProps<"/pay/[orderId]">) {
  return (
    // Anchored near the top, not centred: as the card grows it only extends downwards, like Garden's.
    <main className="relative grid min-h-screen justify-items-center content-start overflow-x-hidden px-4 pt-[max(96px,16vh)] pb-12">
      <Frame />
      {/* A soft light above the card, so the black isn't flat. */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-y-0 left-(--gutter) right-(--gutter) bg-[radial-gradient(50%_40%_at_50%_30%,rgba(255,255,255,0.06),transparent_70%)]"
      />
      <Suspense fallback={<PayCardSkeleton />}>
        {params.then(({ orderId }) => (
          <PayCard orderId={orderId} />
        ))}
      </Suspense>
    </main>
  );
}
