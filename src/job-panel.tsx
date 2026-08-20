import { Fragment, memo, type ReactNode, useMemo } from 'react';

export type JobData = Record<string, unknown>;

const cardClass =
  'mt-7 flex flex-col gap-4 rounded-[14px] border border-[#dde3dc] bg-white p-7 shadow-[0_8px_24px_#1f2a2110]';
const sectionLabelClass =
  'mb-2.5 text-xs font-bold uppercase tracking-[0.08em] text-[#668074]';
const metaClass = 'mt-[3px] mb-0 text-[13px] leading-snug text-[#627067]';
const subMetaClass = 'mt-[3px] mb-0 text-xs leading-snug text-[#627067]';
const jobMetaClass = 'm-0 text-sm leading-snug text-[#627067]';

function string(value: unknown) {
  return typeof value === 'string' && value.trim() ? value.trim() : '';
}

function strings(value: unknown) {
  return Array.isArray(value) ? value.map(string).filter(Boolean) : [];
}

export function safeUrl(value: unknown) {
  const candidate = string(value);
  try {
    const url = new URL(candidate);
    return url.protocol === 'https:' || url.protocol === 'http:'
      ? url.href
      : '';
  } catch {
    return '';
  }
}

function CompanyMark({ company, logo }: { company: string; logo?: unknown }) {
  const logoUrl = safeUrl(logo);
  if (logoUrl) {
    return (
      <img
        className='h-11 w-11 flex-none rounded-[10px] border border-[#dde3dc] bg-white object-contain'
        src={logoUrl}
        alt={`${company} logo`}
      />
    );
  }
  return (
    <span
      className='grid h-11 w-11 flex-none place-items-center rounded-[10px] border border-[#dde3dc] bg-[#eef4ef] text-lg font-extrabold text-[#176a46]'
      aria-hidden='true'
    >
      {company.slice(0, 1).toUpperCase() || '?'}
    </span>
  );
}

function Tags({ values }: { values: string[] }) {
  return values.length ? (
    <div className='flex flex-wrap gap-[7px]'>
      {values.map((value) => (
        <span
          className='rounded-full border border-[#d7e4d9] bg-[#eef4ef] px-[9px] py-1 text-xs text-[#385347]'
          key={value}
        >
          {value}
        </span>
      ))}
    </div>
  ) : null;
}

function renderHtmlNode(node: Node, key: string): ReactNode {
  if (node.nodeType === Node.TEXT_NODE) return node.textContent;
  if (node.nodeType !== Node.ELEMENT_NODE) return null;
  const element = node as HTMLElement;
  const children = Array.from(element.childNodes).map((child, index) =>
    renderHtmlNode(child, `${key}-${index}`),
  );
  switch (element.tagName.toLowerCase()) {
    case 'h2':
      return <h2 key={key}>{children}</h2>;
    case 'h3':
      return <h3 key={key}>{children}</h3>;
    case 'h4':
      return <h4 key={key}>{children}</h4>;
    case 'p':
      return <p key={key}>{children}</p>;
    case 'ul':
      return <ul key={key}>{children}</ul>;
    case 'ol':
      return <ol key={key}>{children}</ol>;
    case 'li':
      return <li key={key}>{children}</li>;
    case 'strong':
    case 'b':
      return <strong key={key}>{children}</strong>;
    case 'em':
    case 'i':
      return <em key={key}>{children}</em>;
    case 'br':
      return <br key={key} />;
    case 'a': {
      const href = safeUrl(element.getAttribute('href'));
      return href ? (
        <a key={key} href={href} target='_blank' rel='noreferrer'>
          {children}
        </a>
      ) : (
        <Fragment key={key}>{children}</Fragment>
      );
    }
    default:
      return <Fragment key={key}>{children}</Fragment>;
  }
}

function RichDescription({ html, text }: { html?: unknown; text?: unknown }) {
  const markup = string(html);
  // Job descriptions run to several kilobytes of HTML. Without memoising, every App
  // render reparses the markup and rebuilds this entire subtree, which reads as a flicker.
  const nodes = useMemo(() => {
    if (!markup) return null;
    const parsed = new DOMParser().parseFromString(markup, 'text/html');
    return Array.from(parsed.body.childNodes).map((node, index) =>
      renderHtmlNode(node, `description-${index}`),
    );
  }, [markup]);
  if (!nodes) return <p>{string(text)}</p>;
  return <>{nodes}</>;
}

function Header({ job, children }: { job: JobData; children?: ReactNode }) {
  const company = string(job.company) || 'Company not provided';
  const source = safeUrl(job.url);
  return (
    <>
      <div className='flex items-start justify-between gap-5 max-[680px]:flex-col'>
        <div className='flex min-w-0 items-center gap-3'>
          <CompanyMark company={company} logo={job.company_logo} />
          <div>
            <p className='m-0 text-base font-bold leading-tight text-[#19221d]'>
              {company}
            </p>
            {children}
          </div>
        </div>
        {source && (
          <a
            className='whitespace-nowrap text-[13px] font-bold text-[#176a46] no-underline'
            href={source}
            target='_blank'
            rel='noreferrer'
          >
            View source -&gt;
          </a>
        )}
      </div>
      <h2 className='m-0 mt-0.5 text-2xl leading-tight text-[#19221d] max-[680px]:text-[21px]'>
        {string(job.title) || 'Job title not provided'}
      </h2>
    </>
  );
}

function Description({ html, text }: { html?: unknown; text?: unknown }) {
  if (!string(html) && !string(text)) return null;
  return (
    <section className='border-t border-[#e7ebe7] pt-[18px]'>
      <h3 className={sectionLabelClass}>Description</h3>
      <div className='rich-description max-h-[430px] overflow-auto pr-2.5 leading-relaxed text-[#38453d] max-[680px]:max-h-[360px]'>
        <RichDescription html={html} text={text} />
      </div>
    </section>
  );
}

function JobFrame({ job, children }: { job: JobData; children: ReactNode }) {
  const warnings = strings(job.warnings);
  return (
    <section className={cardClass}>
      {children}
      {warnings.length > 0 && (
        <p className='m-0 rounded-lg bg-[#fff3eb] px-3 py-2.5 text-[#9e411e]'>
          {warnings.join(' - ')}
        </p>
      )}
    </section>
  );
}

function WelcomeToTheJungleJob({ job }: { job: JobData }) {
  const locations = strings(job.locations);
  return (
    <JobFrame job={job}>
      <Header job={job}>
        <p className={metaClass}>{string(job.company_hq) || locations[0]}</p>
        <p className={subMetaClass}>
          {strings(job.industry_tags).join(' - ')}
        </p>
      </Header>
      <p className={jobMetaClass}>
        {[string(job.job_type), locations.join(' - ')]
          .filter(Boolean)
          .join(' - ')}
      </p>
      <Description html={job.description_html} text={job.description} />
      {string(job.qualifications) && (
        <section>
          <h3 className={sectionLabelClass}>Qualifications</h3>
          <p className='m-0 leading-relaxed text-[#38453d]'>
            {string(job.qualifications)}
          </p>
        </section>
      )}
    </JobFrame>
  );
}

function WellfoundJob({ job }: { job: JobData }) {
  const remoteLocations = strings(job.remote_locations);
  const remote =
    job.remote === true
      ? remoteLocations.length
        ? `Remote (${remoteLocations.join(', ')})`
        : 'Remote'
      : '';
  const experience =
    job.years_experience_min != null || job.years_experience_max != null
      ? `${job.years_experience_min ?? '?'}-${job.years_experience_max ?? '?'} yrs exp`
      : '';
  return (
    <JobFrame job={job}>
      <Header job={job}>
        <p className={metaClass}>
          {[
            string(job.company_hq),
            string(job.company_size) &&
              `${string(job.company_size)} employees`,
          ]
            .filter(Boolean)
            .join(' - ')}
        </p>
        <p className={subMetaClass}>
          {[...strings(job.company_tags), ...strings(job.company_type_tags)]
            .filter(Boolean)
            .join(' - ')}
        </p>
      </Header>
      {string(job.primary_role) && string(job.primary_role) !== string(job.title) && (
        <p className='m-0 text-xs leading-snug text-[#627067]'>
          {string(job.primary_role)}
        </p>
      )}
      <p className={jobMetaClass}>
        {[string(job.job_type), remote, string(job.compensation), experience]
          .filter(Boolean)
          .join(' - ')}
      </p>
      <Tags values={strings(job.skills)} />
      <Tags
        values={[
          job.visa_sponsorship === true ? 'Visa sponsorship' : '',
          job.allow_relocation === true ? 'Relocation' : '',
        ].filter(Boolean)}
      />
      <Description text={job.description} />
      {string(job.company_description) && (
        <p className='m-0 border-t border-[#edf0ed] pt-3.5 text-[13px] italic leading-relaxed text-[#627067]'>
          {string(job.company_description)}
        </p>
      )}
    </JobFrame>
  );
}

function IndeedJob({ job }: { job: JobData }) {
  return (
    <JobFrame job={job}>
      <Header job={job}>
        <p className={metaClass}>{string(job.location)}</p>
      </Header>
      <Description text={job.description} />
    </JobFrame>
  );
}

function GenericJob({ job }: { job: JobData }) {
  const location = string(job.location) || strings(job.locations).join(' - ');
  return (
    <JobFrame job={job}>
      <Header job={job}>
        <p className={metaClass}>{location}</p>
      </Header>
      <p className={jobMetaClass}>{string(job.job_type)}</p>
      <Tags values={strings(job.skills)} />
      <Description
        html={job.description_html}
        text={job.description ?? job.qualifications}
      />
    </JobFrame>
  );
}

// `job` is a stable object identity for the lifetime of a capture, so memoising here
// keeps the whole panel out of the re-render path while a run is in progress.
export const JobPanel = memo(function JobPanel({ job }: { job: JobData }) {
  switch (string(job.domain)) {
    case 'welcometothejungle':
      return <WelcomeToTheJungleJob job={job} />;
    case 'wellfound':
      return <WellfoundJob job={job} />;
    case 'indeed':
      return <IndeedJob job={job} />;
    default:
      return <GenericJob job={job} />;
  }
});
