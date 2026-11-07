import styles from "./card.module.css";

export function Card({ title }: { title: string }) {
  return (
    <section class={styles.combinedCard} data-card="">
      <h2 class={styles.combinedTitle}>{title}</h2>
    </section>
  );
}
