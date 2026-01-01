interface PaginationProps {
  page: number;
  totalItems: number;
  pageSize: number;
  onPageChange: (page: number) => void;
}

export function Pagination({ page, totalItems, pageSize, onPageChange }: PaginationProps) {
  const totalPages = Math.ceil(totalItems / pageSize);
  
  if (totalItems <= pageSize) return null;
  
  return (
    <div className="pagination">
      <span
        className={`page-btn ${page === 0 ? "disabled" : ""}`}
        onClick={() => page > 0 && onPageChange(page - 1)}
      >
        ← prev
      </span>
      <span className="page-info">
        page {page + 1} of {totalPages}
      </span>
      <span
        className={`page-btn ${(page + 1) * pageSize >= totalItems ? "disabled" : ""}`}
        onClick={() => (page + 1) * pageSize < totalItems && onPageChange(page + 1)}
      >
        next →
      </span>
    </div>
  );
}
